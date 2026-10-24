"""Reference implementation of the RoALT link-parsing rules; asserts against links-parse-cases.json."""
import json, re, sys
from urllib.parse import urlsplit, parse_qsl

UUID = re.compile(r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$", re.I)
DIGITS = re.compile(r"^[0-9]{1,64}$")
CODE = re.compile(r"^[A-Za-z0-9_-]{1,128}$")
LOCALE = re.compile(r"^[a-z]{2}(-[a-z0-9]{2,4})?$", re.I)
WEB_HOSTS = {"roblox.com", "www.roblox.com", "web.roblox.com", "m.roblox.com"}
TYPES = {t.lower(): t for t in ["Server", "ExperienceInvite", "NotificationExperienceInvite", "ScreenshotInvite",
                                 "ExperienceDetails", "ExperienceAffiliate", "ExperienceEvent", "Profile",
                                 "FriendInvite", "AvatarItemDetails", "AvatarItemAffiliate", "ContentPost"]}
GAME_TYPES = {"Server", "ExperienceInvite", "NotificationExperienceInvite", "ScreenshotInvite",
              "ExperienceDetails", "ExperienceAffiliate", "ExperienceEvent", "Profile"}
I64 = 2**63 - 1


def bad(r): return {"kind": "Invalid", "reason": r}
def unsupported(r): return {"kind": "Unsupported", "reason": r}


def id_(v):
    if v is None or not re.fullmatch(r"[0-9]{1,19}", v): return None
    n = int(v)
    return n if 0 < n <= I64 else None


def from_query(q):
    """q: dict with lower-cased keys. Shared by /games/start and roblox:// launch links."""
    pid_raw = q.get("placeid", q.get("id"))
    if pid_raw is None:
        if "userid" in q:
            uid = id_(q["userid"])
            return {"kind": "FollowUser", "userId": uid} if uid else bad("userId")
        return bad("placeId")
    pid = id_(pid_raw)
    if not pid: return bad("placeId")
    link = q.get("linkcode", q.get("privateserverlinkcode"))
    if link is not None:
        return {"kind": "PrivateServerLinkCode", "placeId": pid, "linkCode": link} if DIGITS.match(link) else bad("linkCode")
    if "accesscode" in q:
        ac = q["accesscode"]
        return {"kind": "PrivateServerAccessCode", "placeId": pid, "accessCode": ac.lower()} if UUID.match(ac) else bad("accessCode")
    if "gameinstanceid" in q:
        j = q["gameinstanceid"]
        return {"kind": "Instance", "placeId": pid, "jobId": j.lower()} if UUID.match(j) else bad("jobId")
    r = {"kind": "Place", "placeId": pid}
    if q.get("launchdata"): r["launchData"] = q["launchdata"]  # ponytail: no 200-byte check, add if Roblox rejects
    return r


def share(q):
    code = q.get("code")
    if not code or not CODE.match(code): return bad("share code")
    if q.get("v", "").lower() == "v2": return {"kind": "ShareLinkV2", "code": code}
    if "type" not in q: return bad("share type missing")
    t = TYPES.get(q["type"].lower())
    if not t: return bad("share type")
    if t not in GAME_TYPES: return unsupported("not a game link")
    return {"kind": "ShareLink", "code": code, "linkType": t}


def lower_keys(pairs):
    out = {}
    for k, v in pairs:
        out.setdefault(k.lower(), v)  # first occurrence wins
    return out


def parse_deeplink(rest, depth):
    # rest = text after "roblox://"
    path, _, query = rest.partition("?")
    if not query and "=" in path:  # legacy roblox://placeId=1&x=y[/]
        path, query = "", path.rstrip("/")
    q = lower_keys(parse_qsl(query, keep_blank_values=True))
    p = path.strip("/").lower()
    if p in ("", "experiences/start"): return from_query(q)
    if p == "navigation/share_links": return share(q)
    if p == "navigation/game_details":
        u = id_(q.get("gameid"))
        return {"kind": "Universe", "universeId": u} if u else bad("gameId")
    if p == "navigation/profile":
        u = id_(q.get("userid"))
        return {"kind": "User", "userId": u} if u else bad("userId")
    return unsupported("deeplink route")


def parse(s, depth=0):
    s = s.strip().strip("<>\"'").strip()
    if not s: return bad("empty")
    if id_(s): return {"kind": "Place", "placeId": id_(s)}
    if UUID.match(s): return {"kind": "JobIdOnly", "jobId": s.lower()}
    scheme = s.split(":", 1)[0].lower() if ":" in s else ""
    if scheme == "roblox" and s[len("roblox:"):].startswith("//"):
        return parse_deeplink(s[len("roblox://"):], depth)
    if scheme in ("roblox-player", "roblox-studio", "roblox-studio-auth"):
        return bad("launch URI (carries auth ticket) not accepted as input")
    if scheme not in ("http", "https"):
        if scheme: return bad("scheme")
        s = "https://" + s
    u = urlsplit(s)
    host = (u.hostname or "").rstrip(".").lower()
    q = lower_keys(parse_qsl(u.query, keep_blank_values=True))
    if host == "ro.blox.com":
        if depth: return bad("nested AppsFlyer link")
        for k in ("af_dp", "deep_link_value", "af_web_dp"):
            if q.get(k): return parse(q[k], depth + 1)
        return unsupported("AppsFlyer short link without af_dp/deep_link_value/af_web_dp")
    if host not in WEB_HOSTS: return bad("host")
    seg = [x for x in u.path.split("/") if x]
    if len(seg) >= 2 and LOCALE.match(seg[0]) and seg[1].lower() in ("games", "share", "share-links", "users"):
        seg = seg[1:]
    low = [x.lower() for x in seg]
    if low[:2] == ["games", "start"]: return from_query(q)
    if low[:1] == ["games"] and len(seg) >= 2:
        pid = id_(seg[1])
        if not pid: return bad("placeId")
        if "privateserverlinkcode" in q:
            c = q["privateserverlinkcode"]
            return {"kind": "PrivateServerLinkCode", "placeId": pid, "linkCode": c} if DIGITS.match(c) else bad("linkCode")
        if "gameinstanceid" in q:
            j = q["gameinstanceid"]
            return {"kind": "Instance", "placeId": pid, "jobId": j.lower()} if UUID.match(j) else bad("jobId")
        return {"kind": "Place", "placeId": pid}
    if low in (["share"], ["share-links"]): return share(q)
    if low[:1] == ["users"] and len(seg) >= 2:
        uid = id_(seg[1])
        return {"kind": "User", "userId": uid} if uid else bad("userId")
    return unsupported("not a game link")


if __name__ == "__main__":
    cases = json.load(open(sys.argv[1], encoding="utf-8"))["cases"]
    fails = 0
    for c in cases:
        got = parse(c["input"])
        if got != c["expected"]:
            fails += 1
            print("FAIL", repr(c["input"])[:90], "\n  got ", got, "\n  want", c["expected"])
    print(f"{len(cases) - fails}/{len(cases)} pass")
    sys.exit(1 if fails else 0)
