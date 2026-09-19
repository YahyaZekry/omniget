import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...args: unknown[]) => invoke(...args) }));

type ProfileStore = typeof import("./profile-store.svelte");

let store: ProfileStore;

const SAMPLE = {
  public_key_b64: "AAAA",
  fingerprint: "a1b2c3d4e5f60718",
  nickname: "Tonho",
  skin: { id: "omni-default", tint: [90, 169, 255] as [number, number, number] },
  created_at_ms: 1_700_000_000_000,
};

beforeAll(async () => {
  vi.stubGlobal("$state", <T>(value: T) => value);
  vi.stubGlobal("$derived", <T>(value: T) => value);
  store = await import("./profile-store.svelte");
});

afterEach(() => {
  invoke.mockReset();
  store.resetProfileStore();
});

afterAll(() => {
  vi.unstubAllGlobals();
});

describe("nickname normalization", () => {
  it("accepts 1–32 chars and NFC-normalizes", () => {
    expect(store.normalizeNickname("Tonho")).toBe("Tonho");
    expect(store.normalizeNickname("  spaced  ")).toBe("spaced");
    // "e" + combining acute must fold into the precomposed "é".
    expect(store.normalizeNickname("José")).toBe("José");
    expect(store.normalizeNickname("a".repeat(32))).toHaveLength(32);
  });

  it("counts code points, not UTF-16 units, the way Rust chars() does", () => {
    // 32 emoji are 64 UTF-16 units but 32 chars in Rust: both sides accept.
    const thirtyTwoEmoji = "\u{1F600}".repeat(32);
    expect(Array.from(thirtyTwoEmoji)).toHaveLength(32);
    expect(thirtyTwoEmoji.length).toBe(64);
    expect(store.normalizeNickname(thirtyTwoEmoji)).toBe(thirtyTwoEmoji);
    expect(store.normalizeNickname("\u{1F600}".repeat(33))).toBeNull();
  });

  it("trims before rejecting control characters, like normalize_nickname", () => {
    // Rust trims first, so a trailing newline is trimmed away, not rejected.
    expect(store.normalizeNickname("Tonho\n")).toBe("Tonho");
    expect(store.normalizeNickname("To\u0085nho")).toBeNull();
  });

  it("rejects empty, oversized and control characters", () => {
    expect(store.normalizeNickname("")).toBeNull();
    expect(store.normalizeNickname("   ")).toBeNull();
    expect(store.normalizeNickname("a".repeat(33))).toBeNull();
    expect(store.normalizeNickname("bad\nname")).toBeNull();
    expect(store.normalizeNickname("bad\u0000name")).toBeNull();
  });
});

describe("fingerprint formatting", () => {
  it("groups by four and drops existing separators", () => {
    expect(store.formatFingerprint("a1b2c3d4e5f60718")).toBe("a1b2 c3d4 e5f6 0718");
    expect(store.formatFingerprint("a1:b2:c3:d4")).toBe("a1b2 c3d4");
    expect(store.formatFingerprint("abc")).toBe("abc");
    expect(store.formatFingerprint("")).toBe("");
  });
});

describe("tints", () => {
  it("exposes eight palette tints with parsed rgb", () => {
    expect(store.SKIN_TINTS).toHaveLength(8);
    expect(store.SKIN_TINTS.map((t) => t.id)).toContain("blue");
    const blue = store.SKIN_TINTS.find((t) => t.id === "blue");
    expect(blue?.rgb).toEqual([0x5a, 0xa9, 0xff]);
  });

  it("clamps and falls back when turning a tint into CSS", () => {
    expect(store.tintToCss([10, 20, 30])).toBe("rgb(10, 20, 30)");
    expect(store.tintToCss([-5, 999, Number.NaN])).toBe("rgb(0, 255, 0)");
    expect(store.tintToCss(null)).toBe(`rgb(${store.DEFAULT_TINT.join(", ")})`);
    expect(store.tintToCss([1, 2])).toBe(`rgb(${store.DEFAULT_TINT.join(", ")})`);
  });

  it("keeps DEFAULT_TINT aligned with the Rust profile default", () => {
    // src-tauri/src/profile/store.rs: DEFAULT_TINT = [110, 139, 255].
    expect(store.DEFAULT_TINT).toEqual([110, 139, 255]);
    // Deliberately not one of the eight swatches, hence nearestTint.
    expect(store.SKIN_TINTS.some((t) => store.sameTint(t.rgb, store.DEFAULT_TINT))).toBe(false);
  });

  it("picks the nearest swatch by euclidean RGB distance", () => {
    expect(store.nearestTint([0x5a, 0xa9, 0xff]).id).toBe("blue");
    // The Rust default sits between blue and purple, closer to blue.
    expect(store.nearestTint(store.DEFAULT_TINT).id).toBe("blue");
    expect(store.nearestTint([0, 200, 0]).id).toBe("green");
    expect(store.nearestTint([255, 0, 0]).id).toBe("red");
    // Malformed input still selects something instead of nothing.
    expect(store.nearestTint(null).id).toBe(store.nearestTint(store.DEFAULT_TINT).id);
    expect(store.nearestTint([1, 2]).id).toBe(store.nearestTint(store.DEFAULT_TINT).id);
    expect(store.nearestTint([Number.NaN, 0, 0]).id).toBe(store.nearestTint(store.DEFAULT_TINT).id);
  });

  it("compares tints defensively", () => {
    expect(store.sameTint([1, 2, 3], [1, 2, 3])).toBe(true);
    expect(store.sameTint([1, 2, 3], [1, 2, 4])).toBe(false);
    expect(store.sameTint(null, [1, 2, 3])).toBe(false);
  });
});

describe("error mapping", () => {
  it("maps every documented ERR_* code", () => {
    expect(store.profileErrorKey("ERR_PROFILE_NICKNAME")).toBe("profile.err_nickname");
    expect(store.profileErrorKey("ERR_PROFILE_STORE")).toBe("profile.err_store");
    expect(store.profileErrorKey("ERR_PROFILE_SECRET")).toBe("profile.err_secret");
    expect(store.profileErrorKey("ERR_PROFILE_SIGN")).toBe("profile.err_sign");
    expect(store.profileErrorKey("ERR_STUB")).toBe("profile.err_unavailable");
    expect(store.profileErrorKey("boom")).toBe("profile.err_unknown");
    expect(store.profileErrorKey(undefined)).toBe("profile.err_unknown");
  });

  it("treats only ERR_STUB as unavailable", () => {
    expect(store.isUnavailable("ERR_STUB")).toBe(true);
    expect(store.isUnavailable("ERR_PROFILE_STORE")).toBe(false);
  });
});

describe("loading", () => {
  it("loads once per session and caches the profile", async () => {
    invoke.mockResolvedValue(SAMPLE);
    await store.loadProfile();
    await store.loadProfile();
    expect(invoke).toHaveBeenCalledTimes(1);
    expect(invoke).toHaveBeenCalledWith("profile_get");
    expect(store.getProfile()?.nickname).toBe("Tonho");
    expect(store.isProfileAvailable()).toBe(true);
    expect(store.getProfileError()).toBeNull();
  });

  it("reloads when forced", async () => {
    invoke.mockResolvedValue(SAMPLE);
    await store.loadProfile();
    await store.loadProfile(true);
    expect(invoke).toHaveBeenCalledTimes(2);
  });

  it("shows the empty state on ERR_STUB instead of throwing", async () => {
    invoke.mockRejectedValue("ERR_STUB");
    await expect(store.loadProfile()).resolves.toBeUndefined();
    expect(store.getProfile()).toBeNull();
    expect(store.isProfileAvailable()).toBe(false);
    expect(store.getProfileError()).toBe("profile.err_unavailable");
    expect(store.isProfileLoading()).toBe(false);
  });

  it("keeps the section available on a real backend error", async () => {
    invoke.mockRejectedValue("ERR_PROFILE_SECRET");
    await store.loadProfile();
    expect(store.isProfileAvailable()).toBe(true);
    expect(store.getProfileError()).toBe("profile.err_secret");
  });

  it("does not fire a second command while one is in flight", async () => {
    invoke.mockResolvedValue(SAMPLE);
    await Promise.all([store.loadProfile(), store.loadProfile()]);
    expect(invoke).toHaveBeenCalledTimes(1);
  });
});

describe("mutations", () => {
  it("rejects a bad nickname locally, without calling the backend", async () => {
    expect(await store.setNickname("  ")).toBe(false);
    expect(invoke).not.toHaveBeenCalled();
    expect(store.getProfileError()).toBe("profile.err_nickname");
  });

  it("sends the normalized nickname and takes the returned profile", async () => {
    invoke.mockResolvedValueOnce(SAMPLE);
    await store.loadProfile();
    invoke.mockResolvedValueOnce({ ...SAMPLE, nickname: "Novo" });
    expect(await store.setNickname("  Novo  ")).toBe(true);
    expect(invoke).toHaveBeenLastCalledWith("profile_set_nickname", { nick: "Novo" });
    expect(store.getProfile()?.nickname).toBe("Novo");
  });

  it("maps a rejected nickname to its error key", async () => {
    invoke.mockRejectedValue("ERR_PROFILE_NICKNAME");
    expect(await store.setNickname("ok")).toBe(false);
    expect(store.getProfileError()).toBe("profile.err_nickname");
  });

  it("sends id and tint on setSkin and falls back to a local merge", async () => {
    invoke.mockResolvedValueOnce(SAMPLE);
    await store.loadProfile();
    invoke.mockResolvedValueOnce(null);
    expect(await store.setSkin("omni-default", [255, 0, 0])).toBe(true);
    expect(invoke).toHaveBeenLastCalledWith("profile_set_skin", {
      id: "omni-default",
      tint: [255, 0, 0],
    });
    expect(store.getProfile()?.skin.tint).toEqual([255, 0, 0]);
  });

  it("reports a failed setSkin without dropping the profile", async () => {
    invoke.mockResolvedValueOnce(SAMPLE);
    await store.loadProfile();
    invoke.mockRejectedValueOnce("ERR_PROFILE_STORE");
    expect(await store.setSkin("omni-default", [1, 2, 3])).toBe(false);
    expect(store.getProfileError()).toBe("profile.err_store");
    expect(store.getProfile()?.nickname).toBe("Tonho");
  });
});

describe("clipboard", () => {
  it("does nothing without a profile", async () => {
    expect(await store.copyFingerprint()).toBe(false);
  });
});
