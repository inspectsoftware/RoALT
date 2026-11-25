// Wire types come straight from the Rust bindings (ts-rs) so the dashboard can't drift from what the PC sends.
export type { AccountStatus } from "../../../src/bindings/AccountStatus";
export type { BulkItem } from "../../../src/bindings/BulkItem";
export type { BulkStage } from "../../../src/bindings/BulkStage";
export type { ClientMetrics } from "../../../src/bindings/ClientMetrics";
export type { InstanceState } from "../../../src/bindings/InstanceState";
export type { LaunchTarget } from "../../../src/bindings/LaunchTarget";
export type { Limits } from "../../../src/bindings/Limits";
export type { ParsedInput } from "../../../src/bindings/ParsedInput";
export type { RemoteAccount } from "../../../src/bindings/RemoteAccount";
export type { RemoteBulk } from "../../../src/bindings/RemoteBulk";
export type { RemoteCommand } from "../../../src/bindings/RemoteCommand";
export type { RemoteCrash } from "../../../src/bindings/RemoteCrash";
export type { RemoteEvent } from "../../../src/bindings/RemoteEvent";
export type { RemoteFavourite } from "../../../src/bindings/RemoteFavourite";
export type { RemoteGroup } from "../../../src/bindings/RemoteGroup";
export type { RemoteInstance } from "../../../src/bindings/RemoteInstance";
export type { RemotePage } from "../../../src/bindings/RemotePage";
export type { RemotePreviewTarget } from "../../../src/bindings/RemotePreviewTarget";
export type { RemoteRole } from "../../../src/bindings/RemoteRole";
export type { RemoteSnapshot } from "../../../src/bindings/RemoteSnapshot";
export type { RemoteTarget } from "../../../src/bindings/RemoteTarget";
export type { SystemUsage } from "../../../src/bindings/SystemUsage";

export type PcConnStatus = "connecting" | "online" | "offline" | "tooMany" | "tooManyTabs" | "paused" | "deviceBusy";

export type PreviewFrame = {
  instanceId: string;
  seq: number;
  ageMs: number;
  status: number;
  picture: boolean;
};
