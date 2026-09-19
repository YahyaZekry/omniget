<script lang="ts">
  /**
   * The avatar as the `/llm` rail and the profile card use it: Omni in his tinted
   * disc, plus the two states a rail needs to show at a glance — selected and
   * speaking. Everything is CSS; the component itself holds no state and starts
   * no timer, so a rail with 50 of these costs nothing while nobody talks.
   */
  import Avatar from "./Avatar.svelte";
  import type { Mood } from "$lib/omni/skins";

  let {
    tint,
    size = 36,
    mood = "neutral",
    speaking = false,
    active = false,
    label,
  }: {
    tint: readonly number[] | null | undefined;
    size?: number;
    mood?: Mood | string;
    speaking?: boolean;
    /** The rail item is the selected one. */
    active?: boolean;
    label?: string;
  } = $props();
</script>

<span
  class="rail-avatar"
  class:active
  class:speaking
  style:--rail-avatar-size="{size}px"
>
  <Avatar {tint} {size} {mood} {speaking} {label} />
</span>

<style>
  .rail-avatar {
    position: relative;
    display: inline-flex;
    width: var(--rail-avatar-size);
    height: var(--rail-avatar-size);
    flex-shrink: 0;
    border-radius: var(--radius-full);
  }

  /* Selection and speaking rings live on the wrapper so they can sit OUTSIDE the
     disc, which clips its own sprite. */
  .rail-avatar::after {
    content: "";
    position: absolute;
    inset: -3px;
    border-radius: var(--radius-full);
    box-shadow: 0 0 0 0 transparent;
    transition: box-shadow var(--duration-fast) var(--ease-out);
    pointer-events: none;
  }

  .rail-avatar.active::after {
    box-shadow: 0 0 0 2px var(--accent);
  }

  .rail-avatar.speaking::after {
    box-shadow: 0 0 0 2px var(--accent);
    animation: rail-avatar-pulse 1.4s var(--ease-in-out, ease-in-out) infinite;
  }

  @keyframes rail-avatar-pulse {
    0%,
    100% {
      opacity: 0.35;
    }
    50% {
      opacity: 1;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .rail-avatar::after {
      transition: none;
    }

    .rail-avatar.speaking::after {
      /* Still readable as "this one is talking", just not moving. */
      animation: none;
      opacity: 1;
    }
  }
</style>
