import { describe, expect, it } from "vitest";
import {
  buildHandshake,
  bytesToUuid,
  deriveManualPairKeys,
  deriveQrPairKey,
  DIR_PC_TO_VIEWER,
  DIR_VIEWER_TO_PC,
  fromB64Url,
  generateEphemeralEcdh,
  generateIdentityKeyPair,
  openPairBlob,
  pairCommit,
  PAIR_MODE_MANUAL,
  PAIR_MODE_QR,
  pairTranscript,
  sealPairBlob,
  SessionCipher,
  signDeviceBinding,
  toB64Url,
  uuidToBytes,
  verifyHandshake,
  verifyPcBinding,
} from "./crypto";
import { newDeviceIdentity } from "./storage";
import { isEnrolledBlob, qrPairFlow, readPairAck } from "./transport";

const enc = new TextEncoder();
const dec = new TextDecoder();
const hex = (u: Uint8Array) => Array.from(u, (b) => b.toString(16).padStart(2, "0")).join("");

describe("web/remote E2EE crypto", () => {
  it("round-trips UUID bytes", () => {
    const id = "9b2f6c1e-3d4a-4b5c-8d6e-7f8091a2b3c4";
    expect(bytesToUuid(uuidToBytes(id))).toBe(id);
  });

  it("performs mutual P-256 handshake, encrypts bidirectionally, and blocks replay & tampering", async () => {
    const pcId = "11111111-2222-4333-8444-555555555555";
    const devId = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";

    const pcStatic = await generateIdentityKeyPair();
    const viewerStatic = await generateIdentityKeyPair();
    const pcEphem = await generateEphemeralEcdh();
    const viewerEphem = await generateEphemeralEcdh();

    const pcHs = await buildHandshake(pcStatic.keyPair.privateKey, pcEphem.publicKeyRaw, DIR_PC_TO_VIEWER, pcId, devId);
    const viewerHs = await buildHandshake(viewerStatic.keyPair.privateKey, viewerEphem.publicKeyRaw, DIR_VIEWER_TO_PC, pcId, devId);

    expect(pcHs.length).toBe(130);
    expect(viewerHs.length).toBe(130);

    const verifiedPcPub = await verifyHandshake(pcStatic.publicKeyRaw, pcHs, DIR_PC_TO_VIEWER, pcId, devId);
    const verifiedViewerPub = await verifyHandshake(viewerStatic.publicKeyRaw, viewerHs, DIR_VIEWER_TO_PC, pcId, devId);

    // Wrong role or wrong static pubkey must throw.
    await expect(verifyHandshake(viewerStatic.publicKeyRaw, pcHs, DIR_PC_TO_VIEWER, pcId, devId)).rejects.toThrow();
    await expect(verifyHandshake(pcStatic.publicKeyRaw, pcHs, DIR_VIEWER_TO_PC, pcId, devId)).rejects.toThrow();

    const pcCipher = await SessionCipher.derive(pcEphem.keyPair.privateKey, verifiedViewerPub, verifiedPcPub, verifiedViewerPub, true);
    const viewerCipher = await SessionCipher.derive(viewerEphem.keyPair.privateKey, verifiedPcPub, verifiedPcPub, verifiedViewerPub, false);

    // PC -> Viewer
    const ct1 = await pcCipher.seal(new TextEncoder().encode("snapshot-payload"));
    const pt1 = await viewerCipher.open(ct1);
    expect(new TextDecoder().decode(pt1)).toBe("snapshot-payload");

    // Replay rejected!
    await expect(viewerCipher.open(ct1)).rejects.toThrow(/Replay/);

    // Tampered ciphertext rejected!
    const ct2 = await pcCipher.seal(new TextEncoder().encode("delta"));
    ct2[ct2.length - 1] ^= 0xff;
    await expect(viewerCipher.open(ct2)).rejects.toThrow();

    // Viewer -> PC
    const cmdCt = await viewerCipher.seal(new TextEncoder().encode('{"op":"panicKill"}'));
    expect(new TextDecoder().decode(await pcCipher.open(cmdCt))).toBe('{"op":"panicKill"}');
  });

  it("seals pairing blobs per direction and derives the manual SAS over the transcript", async () => {
    const pairId = "a1b2c3d4e5f60708";
    const secret = crypto.getRandomValues(new Uint8Array(32));
    const k = await deriveQrPairKey(secret, pairId);
    const blob = await sealPairBlob(k, DIR_VIEWER_TO_PC, pairId, enc.encode('{"deviceId":"test"}'));
    expect(dec.decode(await openPairBlob(k, DIR_VIEWER_TO_PC, pairId, blob))).toBe('{"deviceId":"test"}');
    await expect(openPairBlob(k, DIR_VIEWER_TO_PC, "wrong-pair-id", blob)).rejects.toThrow();
    await expect(openPairBlob(k, DIR_PC_TO_VIEWER, pairId, blob)).rejects.toThrow();

    const a = await generateEphemeralEcdh();
    const b = await generateEphemeralEcdh();
    const nonce = crypto.getRandomValues(new Uint8Array(32));
    const th = await pairTranscript(PAIR_MODE_MANUAL, pairId, a.publicKeyRaw, b.publicKeyRaw, nonce);
    const resA = await deriveManualPairKeys(a.keyPair.privateKey, b.publicKeyRaw, th);
    const resB = await deriveManualPairKeys(b.keyPair.privateKey, a.publicKeyRaw, th);
    expect(resA.sas).toBe(resB.sas);
    expect(resA.sas).toMatch(/^\d{3} \d{3}$/);
    const ackBlob = await sealPairBlob(resA.pairKey, DIR_PC_TO_VIEWER, pairId, enc.encode("room-token"));
    expect(dec.decode(await openPairBlob(resB.pairKey, DIR_PC_TO_VIEWER, pairId, ackBlob))).toBe("room-token");

    const otherTh = await pairTranscript(PAIR_MODE_MANUAL, pairId, a.publicKeyRaw, b.publicKeyRaw, crypto.getRandomValues(new Uint8Array(32)));
    await expect(openPairBlob((await deriveManualPairKeys(b.keyPair.privateKey, a.publicKeyRaw, otherTh)).pairKey, DIR_PC_TO_VIEWER, pairId, ackBlob)).rejects.toThrow();
  });

  it("a QR pairing answers only an ack sealed under its QR key and signed over its own transcript", async () => {
    const pairId = "00112233445566778899aabbccddeeff";
    const secret = new Uint8Array(32).fill(5);
    await expect(qrPairFlow("wss://r", "ABCD2345", toB64Url(secret), "Phone")).rejects.toThrow(/not valid/);
    await expect(qrPairFlow("wss://r", pairId.toUpperCase(), toB64Url(secret), "Phone")).rejects.toThrow(/not valid/);
    const flow = await qrPairFlow("wss://r", pairId, toB64Url(secret), "Phone");
    const key = await deriveQrPairKey(secret, pairId);
    const first = flow.first.msg as { nonceB64: string; blob: string };
    const th = await pairTranscript(PAIR_MODE_QR, pairId, new Uint8Array(), new Uint8Array(), fromB64Url(first.nonceB64));
    const { deviceId } = JSON.parse(dec.decode(await openPairBlob(key, DIR_VIEWER_TO_PC, pairId, first.blob))) as { deviceId: string };
    const pcId = "11111111-2222-4333-8444-555555555555";
    const pc = await generateIdentityKeyPair();
    const sign = async (transcript: Uint8Array) =>
      toB64Url(
        new Uint8Array(
          await crypto.subtle.sign(
            { name: "ECDSA", hash: "SHA-256" },
            pc.keyPair.privateKey,
            new Uint8Array([...enc.encode("roalt-remote-v2-pc"), ...transcript, ...uuidToBytes(pcId), ...pc.publicKeyRaw, ...uuidToBytes(deviceId)]),
          ),
        ),
      );
    const body = { pcId, pcName: "Gaming PC", pcPubB64: toB64Url(pc.publicKeyRaw), roomToken: "A".repeat(43), role: "control", sigB64: await sign(th) };
    const ack = async (b: object, k = key) => ({ mode: "ack", blob: await sealPairBlob(k, DIR_PC_TO_VIEWER, pairId, enc.encode(JSON.stringify(b))) });
    const join = () => {
      throw new Error("the QR flow must not join again");
    };
    const offer = (outer: object) => flow.step(outer, join).catch(() => null);

    expect(await offer({ mode: "manual_challenge", pairId, pcEphemPubB64: toB64Url(new Uint8Array(65).fill(4)) })).toBeNull();
    expect(await offer(await ack(body, await deriveQrPairKey(new Uint8Array(32).fill(6), pairId)))).toBeNull();
    const otherTh = await pairTranscript(PAIR_MODE_MANUAL, pairId, new Uint8Array(), new Uint8Array(), fromB64Url(first.nonceB64));
    expect(await offer(await ack({ ...body, sigB64: await sign(otherTh) }))).toBeNull();
    expect((await offer(await ack(body)))?.pc).toMatchObject({ pcId, deviceId, role: "control" });
  });

  it("binds the PC key only to its own transcript and device", async () => {
    const pcId = "11111111-2222-4333-8444-555555555555";
    const devId = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
    const dev = await generateIdentityKeyPair();
    const th = await pairTranscript(PAIR_MODE_QR, "0123456789abcdef", new Uint8Array(), new Uint8Array(), new Uint8Array(32).fill(7));
    const sig = await signDeviceBinding(dev.keyPair.privateKey, th, devId, dev.publicKeyRaw);
    expect(sig.length).toBe(64);
    expect(await verifyPcBinding(dev.publicKeyRaw, th, pcId, devId, sig)).toBe(false);
    expect(await verifyPcBinding(dev.publicKeyRaw.slice(1), th, pcId, devId, sig)).toBe(false);
  });

  it("stores a pairing ack only when it is well-formed and signed by its PC for this transcript", async () => {
    const pairId = "0123456789abcdef";
    const pcId = "11111111-2222-4333-8444-555555555555";
    const key = await deriveQrPairKey(new Uint8Array(32).fill(9), pairId);
    const th = await pairTranscript(PAIR_MODE_QR, pairId, new Uint8Array(), new Uint8Array(), new Uint8Array(32).fill(1));
    const pc = await generateIdentityKeyPair();
    const device = await newDeviceIdentity();
    const pcSig = async (deviceId: string) =>
      toB64Url(
        new Uint8Array(
          await crypto.subtle.sign(
            { name: "ECDSA", hash: "SHA-256" },
            pc.keyPair.privateKey,
            new Uint8Array([...enc.encode("roalt-remote-v2-pc"), ...th, ...uuidToBytes(pcId), ...pc.publicKeyRaw, ...uuidToBytes(deviceId)]),
          ),
        ),
      );
    const good = { pcId, pcName: "Gaming PC", pcPubB64: toB64Url(pc.publicKeyRaw), roomToken: "A".repeat(43), role: "admin", sigB64: await pcSig(device.deviceId) };
    const ack = (body: object, dir = DIR_PC_TO_VIEWER) => sealPairBlob(key, dir, pairId, enc.encode(JSON.stringify(body)));

    const saved = await readPairAck(key, pairId, await ack(good), th, device, "wss://relay");
    expect(saved).toMatchObject({ pcId, pcName: "Gaming PC", role: "readonly", deviceId: device.deviceId, relayUrl: "wss://relay" });
    expect((await readPairAck(key, pairId, await ack({ ...good, role: "control" }), th, device, "wss://relay")).role).toBe("control");
    const emoji = "\u{1F3AE}".repeat(48);
    expect((await readPairAck(key, pairId, await ack({ ...good, pcName: emoji }), th, device, "wss://relay")).pcName).toBe(emoji);
    expect((await readPairAck(key, pairId, await ack({ ...good, pcName: "Gaming​ PC‮" }), th, device, "wss://relay")).pcName).toBe("Gaming PC");

    for (const bad of [{ pcId: "not-a-uuid" }, { roomToken: 5 }, { pcName: "" }, { sigB64: await pcSig(crypto.randomUUID()) }, { pcPubB64: toB64Url(device.publicKeyRaw) }]) {
      await expect(readPairAck(key, pairId, await ack({ ...good, ...bad }), th, device, "wss://relay")).rejects.toThrow();
    }
    await expect(readPairAck(key, pairId, await ack(good, DIR_VIEWER_TO_PC), th, device, "wss://relay")).rejects.toThrow();
    const otherTh = await pairTranscript(PAIR_MODE_QR, pairId, new Uint8Array(), new Uint8Array(), new Uint8Array(32).fill(2));
    await expect(readPairAck(key, pairId, await ack(good), otherTh, device, "wss://relay")).rejects.toThrow();
  });

  it("interoperates byte-for-byte with Rust src-tauri/src/remote/crypto.rs test vectors", async () => {
    const pcId = "11111111-2222-4333-8444-555555555555";
    const devId = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
    const pairId = "0123456789abcdef";

    const pcStaticPub = fromB64Url("BAIX5hfwtkQ5KCePlpmeaaI6TywVK99tbN9m5bgCgtTtGUp968uXcS0t2jyoWqh2Wlb0X8dYWZZS8ol8ZTBuV5Q");
    const pcHs = fromB64Url(
      "AQTWWpOXfKo9GwgYUv9Xp55GXxZgV3MEuurVBd06SFic81AYXolTct9iIeo6E3VX5HP922dV8FvVB8PFM_zpyRKFh0WGmYWxHxuvWsRuSPySACGb2ySSuApgzYbOyrZGRdpz00GFgdJ9mL1I6jwQv3Bn9-cPw7Ne4_CSbXkqu0byTg",
    );
    const viewerEphemPub = fromB64Url("BFGnWAgziY6hsYPL1zUKQJkHjG7xweGOlwzXaDA18l59ARBSJxKwtafP8IFoVIaYSpTmgx7axG5zYPqdg0p6gaE");
    const rustFrame = fromB64Url("AgAAAAAAAAABGz9lADf3exgLBceQ8mcZIQplYmM_OlVj3SLHXgD2QA");
    const rustQrBlob = "JtM7ujKBI809qllUdXgVYlyGrSBpY-Ik3XLmuNgACXTl";
    const rustManualBlob = "nxrFIADmScVoH9RN6lyVp6uh0YdEFM3n22DC05R5Yta2xxVc_A";
    const rustPcBindSig = fromB64Url("lgHJXVPUMByxy8ADSyM9a0J4dJPNY4ql_oeg7HjMppZliQ-9qae_rc-O9z1uSYWjyi-GzLYEGPe3WStjqTXMSg");
    const webDevicePub = new Uint8Array(fromB64Url("BBec6s4dd5vbaoX71zsuDENxCZE5RZPn8rRA4ZYmjlwTS2nbg9daAIOyTgO-9o6ZjZ8sbxk0BgUz0nLr0MlhKrw"));
    const webDeviceSig = new Uint8Array(fromB64Url("p28FIx4nF0_p4xyPOY1e2S5TdogJQLnpmKMU4B0itVOYcOeJ1McD5GBBcSBUIc8wq42bS_rIXx749FMe83CMnA"));
    const webManualBlob = "XDlLyVJvYt_mSIR6B6r8X31Lsj5CFq9lcZco-4oydVhuQg";
    const nonce = new Uint8Array(32).fill(0x55);

    // 1. Verify Rust's ECDSA P-256 handshake frame.
    const pcEphemPub = await verifyHandshake(pcStaticPub, pcHs, DIR_PC_TO_VIEWER, pcId, devId);

    // 2. Import viewer's deterministic P-256 ECDH private key (scalar 0x33 * 32) via JWK.
    const viewerPriv = await crypto.subtle.importKey(
      "jwk",
      {
        kty: "EC",
        crv: "P-256",
        x: toB64Url(viewerEphemPub.slice(1, 33)),
        y: toB64Url(viewerEphemPub.slice(33, 65)),
        d: toB64Url(new Uint8Array(32).fill(0x33)),
      },
      { name: "ECDH", namedCurve: "P-256" },
      false,
      ["deriveBits"],
    );

    // 3. Derive session cipher and decrypt Rust's AES-256-GCM frame.
    const viewerCipher = await SessionCipher.derive(viewerPriv, pcEphemPub, pcEphemPub, viewerEphemPub, false);
    const decrypted = await viewerCipher.open(rustFrame);
    expect(new TextDecoder().decode(decrypted)).toBe("hello from rust");

    expect(hex(await pairCommit(viewerEphemPub, nonce))).toBe("e179054dbf57cebe74ee5be8ed103a7f47072eedb5e3e036475be4ba0d6c998f");
    const qrTh = await pairTranscript(PAIR_MODE_QR, pairId, new Uint8Array(), new Uint8Array(), nonce);
    expect(hex(qrTh)).toBe("76d48323f8391c641079a1d56f6113e1de94ccd31ae6b2c1042a5f5b52478513");
    const manualTh = await pairTranscript(PAIR_MODE_MANUAL, pairId, pcEphemPub, viewerEphemPub, nonce);
    expect(hex(manualTh)).toBe("579f983dd0316b473126e5622174c23bb57919fdee3a740b863c8949ad8cc281");
    const { sas, pairKey } = await deriveManualPairKeys(viewerPriv, pcEphemPub, manualTh);
    expect(sas).toBe("788 711");
    expect(dec.decode(await openPairBlob(pairKey, DIR_PC_TO_VIEWER, pairId, rustManualBlob))).toBe("manual-ok");
    await expect(openPairBlob(pairKey, DIR_VIEWER_TO_PC, pairId, rustManualBlob)).rejects.toThrow();
    expect(await isEnrolledBlob(pairKey, pairId, "7A_-ncL568EkElXtV89LuIgN9NvikzBf5BaJNHNBemUXuhT_ka4CTkJx0VYJocs")).toBe(true);
    expect(await isEnrolledBlob(pairKey, pairId, rustManualBlob)).toBe(false);
    expect(await isEnrolledBlob(pairKey, "fedcba9876543210", "7A_-ncL568EkElXtV89LuIgN9NvikzBf5BaJNHNBemUXuhT_ka4CTkJx0VYJocs")).toBe(false);
    const forged = await sealPairBlob(pairKey, DIR_VIEWER_TO_PC, pairId, enc.encode('{"mode":"enrolled"}'));
    expect(await isEnrolledBlob(pairKey, pairId, forged)).toBe(false);
    expect(await isEnrolledBlob(pairKey, pairId, undefined)).toBe(false);
    expect(await verifyPcBinding(pcStaticPub, manualTh, pcId, devId, rustPcBindSig)).toBe(true);
    expect(await verifyPcBinding(pcStaticPub, qrTh, pcId, devId, rustPcBindSig)).toBe(false);
    expect(dec.decode(await openPairBlob(pairKey, DIR_VIEWER_TO_PC, pairId, webManualBlob))).toBe("web-ok");
    const deviceKey = await crypto.subtle.importKey("raw", webDevicePub, { name: "ECDSA", namedCurve: "P-256" }, false, ["verify"]);
    const deviceBinding = (th: Uint8Array) => new Uint8Array([...enc.encode("roalt-remote-v2-device"), ...th, ...uuidToBytes(devId), ...webDevicePub]);
    expect(await crypto.subtle.verify({ name: "ECDSA", hash: "SHA-256" }, deviceKey, webDeviceSig, deviceBinding(manualTh))).toBe(true);
    expect(await crypto.subtle.verify({ name: "ECDSA", hash: "SHA-256" }, deviceKey, webDeviceSig, deviceBinding(qrTh))).toBe(false);
    const signer = await generateIdentityKeyPair();
    const signed = await signDeviceBinding(signer.keyPair.privateKey, manualTh, devId, signer.publicKeyRaw);
    const signerBinding = new Uint8Array([...enc.encode("roalt-remote-v2-device"), ...manualTh, ...uuidToBytes(devId), ...signer.publicKeyRaw]);
    expect(await crypto.subtle.verify({ name: "ECDSA", hash: "SHA-256" }, signer.keyPair.publicKey, new Uint8Array(signed), signerBinding)).toBe(true);

    // 4. Decrypt Rust's QR pairing blob.
    const qrKey = await deriveQrPairKey(new Uint8Array(32).fill(0x44), pairId);
    expect(dec.decode(await openPairBlob(qrKey, DIR_PC_TO_VIEWER, pairId, rustQrBlob))).toBe("qr-ok");
  });
});
