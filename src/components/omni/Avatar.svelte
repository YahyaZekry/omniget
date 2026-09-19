<script lang="ts">
  /**
   * Omni's face: the mascot sprite inside a tinted disc.
   *
   * The animation is one CSS `steps()` walk over a background-position, so there
   * is no JavaScript per frame, no `requestAnimationFrame` and no timer: when the
   * avatar is neither hovered nor speaking the element is completely idle, and
   * `prefers-reduced-motion` freezes it on frame 0.
   *
   * Omni is green by nature, so the skin tint is NOT painted on the character: it
   * colours the disc behind him and its ring, the same way `ProfileCard` used the
   * tint for its placeholder disc. That keeps 8 skins readable at 24 px while the
   * mascot stays recognizable.
   */
  import {
    OMNI_STRIP,
    moodPose,
    stripAnimationCss,
    type Mood,
    type OmniPose,
  } from "$lib/omni/skins";
  import { tintToCss } from "$lib/stores/profile-store.svelte";

  let {
    tint,
    size = 44,
    mood = "neutral",
    speaking = false,
    animate = "auto",
    pose,
    label,
  }: {
    /** RGB 0-255, straight from `profile.skin.tint` or `SKIN_TINTS`. */
    tint: readonly number[] | null | undefined;
    /** Side of the disc in CSS px. */
    size?: number;
    mood?: Mood | string;
    speaking?: boolean;
    /** `auto` animates on hover or while speaking; `always`/`never` force it. */
    animate?: "auto" | "always" | "never";
    /** Overrides the pose the mood would pick (used by the gallery/tests). */
    pose?: OmniPose;
    /** Accessible name. Without one the avatar is decoration and is hidden. */
    label?: string;
  } = $props();

  let activePose = $derived(pose ?? moodPose(mood, speaking));
  let anim = $derived(stripAnimationCss(OMNI_STRIP, activePose));
  // The sprite is 48x64 with the pivot at (24,60): drawn at 1:1 and anchored to
  // the bottom centre of the disc, the feet sit on the rim and the head fills the
  // top, which is what reads best in a circle. `scale` is the only size knob.
  let scale = $derived(size / 40);
</script>

<span
  class="omni-avatar"
  style:--omni-size="{size}px"
  style:--omni-tint={tintToCss(tint)}
  style:--omni-image="url({anim.image})"
  style:--omni-sheet-w="{anim.widthPx}px"
  style:--omni-frame-w="{anim.frameW}px"
  style:--omni-frame-h="{anim.frameH}px"
  style:--omni-from="{anim.fromPx}px"
  style:--omni-to="{anim.toPx}px"
  style:--omni-steps={anim.steps}
  style:--omni-duration="{anim.durationMs}ms"
  style:--omni-scale={scale}
  data-mood={mood}
  data-pose={activePose}
  data-animate={animate}
  data-speaking={speaking ? "true" : "false"}
  role={label ? "img" : "presentation"}
  aria-label={label}
  aria-hidden={label ? undefined : "true"}
>
  <span class="omni-sprite"></span>
</span>

<style>
  .omni-avatar {
    position: relative;
    display: inline-block;
    width: var(--omni-size);
    height: var(--omni-size);
    flex-shrink: 0;
    border-radius: var(--radius-full);
    overflow: hidden;
    background:
      radial-gradient(
        circle at 50% 22%,
        rgba(255, 255, 255, 0.34),
        rgba(255, 255, 255, 0) 62%
      ),
      var(--omni-tint, #6e8bff);
    box-shadow:
      inset 0 0 0 var(--hairline) rgba(0, 0, 0, 0.18),
      var(--elev-1);
  }

  .omni-sprite {
    position: absolute;
    left: 50%;
    bottom: 0;
    width: var(--omni-frame-w);
    height: var(--omni-frame-h);
    transform-origin: bottom center;
    transform: translateX(-50%) scale(var(--omni-scale));
    background-image: var(--omni-image);
    background-repeat: no-repeat;
    background-size: var(--omni-sheet-w) var(--omni-frame-h);
    background-position: var(--omni-from) 0;
    /* Pixel art: never let the compositor smooth it. */
    image-rendering: pixelated;
    pointer-events: none;
  }

  /*
   * One keyframe pair for every pose: the custom properties carry the pose's
   * slice of the strip, so changing mood changes four variables and no CSS rule.
   * `to` is one frame PAST the last one, which is what makes `steps(n)` land on
   * each frame exactly once.
   */
  @keyframes omni-walk {
    from {
      background-position-x: var(--omni-from);
    }
    to {
      background-position-x: var(--omni-to);
    }
  }

  .omni-avatar[data-animate="always"] .omni-sprite,
  .omni-avatar[data-animate="auto"][data-speaking="true"] .omni-sprite {
    animation: omni-walk var(--omni-duration) steps(var(--omni-steps)) infinite;
  }

  @media (hover: hover) {
    .omni-avatar[data-animate="auto"]:hover .omni-sprite {
      animation: omni-walk var(--omni-duration) steps(var(--omni-steps)) infinite;
    }
  }

  /* Reduced motion: frame 0, no animation at all, in every state. */
  @media (prefers-reduced-motion: reduce) {
    .omni-avatar .omni-sprite,
    .omni-avatar[data-animate="always"] .omni-sprite,
    .omni-avatar[data-animate="auto"][data-speaking="true"] .omni-sprite,
    .omni-avatar[data-animate="auto"]:hover .omni-sprite {
      animation: none;
      background-position: var(--omni-from) 0;
    }
  }
</style>
