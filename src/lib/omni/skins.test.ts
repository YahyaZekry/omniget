import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import {
  MOODS,
  OMNI_STRIP,
  STRIP_POSES,
  framePositionPx,
  moodPose,
  poseSlice,
  stripAnimationCss,
  stripFromAtlas,
  type OmniAtlas,
  type OmniPose,
} from "./skins";

const atlas: OmniAtlas = JSON.parse(
  readFileSync(
    fileURLToPath(new URL("../../../static/world/omni/atlas.json", import.meta.url)),
    "utf8",
  ),
);

describe("moodPose", () => {
  it("maps every mood to a pose that exists in the strip", () => {
    for (const mood of MOODS) {
      const pose = moodPose(mood);
      expect(STRIP_POSES).toContain(pose);
      expect(OMNI_STRIP.poses[pose]).toBeDefined();
    }
  });

  it("uses the poses the design asks for", () => {
    expect(moodPose("neutral")).toBe("idle");
    expect(moodPose("curious")).toBe("idle");
    expect(moodPose("worried")).toBe("idle");
    expect(moodPose("happy")).toBe("wave");
    expect(moodPose("proud")).toBe("wave");
    expect(moodPose("sleepy")).toBe("sleep");
    expect(moodPose("focused")).toBe("work");
  });

  it("falls back to idle for an unknown or missing mood", () => {
    expect(moodPose(undefined)).toBe("idle");
    expect(moodPose(null)).toBe("idle");
    expect(moodPose("dancing")).toBe("idle");
  });

  it("talks while speaking, whatever the mood", () => {
    for (const mood of MOODS) {
      expect(moodPose(mood, true)).toBe("talk");
    }
    expect(moodPose("sleepy", true)).toBe("talk");
  });
});

describe("framePositionPx", () => {
  it("walks the strip one frame width at a time", () => {
    expect(framePositionPx(OMNI_STRIP, "idle", 0)).toBe(0);
    expect(framePositionPx(OMNI_STRIP, "idle", 1)).toBe(-48);
    expect(framePositionPx(OMNI_STRIP, "idle", 3)).toBe(-144);
  });

  it("offsets by the pose start", () => {
    // talk starts at frame 4 of the strip.
    expect(framePositionPx(OMNI_STRIP, "talk", 0)).toBe(-192);
    expect(framePositionPx(OMNI_STRIP, "talk", 2)).toBe(-288);
    // work is the last pose: 18 + 7 = frame 25, the last of 26.
    expect(framePositionPx(OMNI_STRIP, "work", 7)).toBe(-25 * 48);
  });

  it("clamps inside the pose so it never bleeds into the next one", () => {
    const talk = OMNI_STRIP.poses.talk;
    expect(framePositionPx(OMNI_STRIP, "talk", 99)).toBe(
      -(talk.start + talk.count - 1) * 48,
    );
    expect(framePositionPx(OMNI_STRIP, "talk", -5)).toBe(-talk.start * 48);
    expect(framePositionPx(OMNI_STRIP, "talk", Number.NaN)).toBe(-talk.start * 48);
    expect(framePositionPx(OMNI_STRIP, "talk", 1.9)).toBe(-(talk.start + 1) * 48);
  });

  it("treats an unknown pose as idle", () => {
    expect(poseSlice(OMNI_STRIP, "moonwalk")).toEqual(OMNI_STRIP.poses.idle);
    expect(framePositionPx(OMNI_STRIP, "moonwalk", 2)).toBe(-96);
  });

  it("never points past the strip for any pose and frame", () => {
    const limit = -(OMNI_STRIP.frames - 1) * OMNI_STRIP.frameW;
    for (const pose of STRIP_POSES) {
      const slice = OMNI_STRIP.poses[pose];
      for (let i = 0; i < slice.count; i += 1) {
        const x = framePositionPx(OMNI_STRIP, pose, i);
        expect(x).toBeLessThanOrEqual(0);
        expect(x).toBeGreaterThanOrEqual(limit);
        expect(Math.abs(x % OMNI_STRIP.frameW)).toBe(0);
      }
    }
  });
});

describe("stripAnimationCss", () => {
  it("ends one frame past the last one so steps() lands on each frame", () => {
    const idle = stripAnimationCss(OMNI_STRIP, "idle");
    expect(idle.fromPx).toBe(0);
    expect(idle.toPx).toBe(-4 * 48);
    expect(idle.steps).toBe(4);
    // 4 frames at 6 fps.
    expect(idle.durationMs).toBe(667);
    expect(idle.widthPx).toBe(26 * 48);
  });

  it("starts where the pose starts", () => {
    const wave = stripAnimationCss(OMNI_STRIP, "wave");
    expect(wave.fromPx).toBe(framePositionPx(OMNI_STRIP, "wave", 0));
    expect(wave.toPx).toBe(wave.fromPx - wave.steps * 48);
    expect(wave.steps).toBe(6);
    expect(wave.durationMs).toBe(600);
  });

  it("gives every pose a positive duration and the strip width", () => {
    for (const pose of STRIP_POSES) {
      const css = stripAnimationCss(OMNI_STRIP, pose);
      expect(css.durationMs).toBeGreaterThan(0);
      expect(css.widthPx).toBe(OMNI_STRIP.frames * OMNI_STRIP.frameW);
      expect(css.image).toBe(OMNI_STRIP.image);
    }
  });
});

describe("stripFromAtlas", () => {
  it("reproduces the shipped descriptor from the real world atlas", () => {
    // Drift guard: if the art is repacked, `build-strip.py` has to run again.
    expect(stripFromAtlas(atlas)).toEqual(OMNI_STRIP);
  });

  it("refuses a mirrored direction", () => {
    // E is declared as mirror_of W in the atlas, it has no frames of its own.
    expect(() => stripFromAtlas(atlas, ["idle"], "E")).toThrow(/mirror/);
  });

  it("refuses a pose the atlas does not have", () => {
    expect(() => stripFromAtlas(atlas, ["breakdance" as OmniPose])).toThrow(
      /no animation/,
    );
  });

  it("refuses frames of mixed sizes", () => {
    const broken: OmniAtlas = JSON.parse(JSON.stringify(atlas));
    broken.frames["omni/idle/S/1"].w = 32;
    expect(() => stripFromAtlas(broken, ["idle"])).toThrow(/expected 48x64/);
  });

  it("refuses a dangling frame key", () => {
    const broken: OmniAtlas = JSON.parse(JSON.stringify(atlas));
    delete broken.frames["omni/idle/S/2"];
    expect(() => stripFromAtlas(broken, ["idle"])).toThrow(/no frame/);
  });
});
