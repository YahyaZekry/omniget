<script lang="ts">
  /**
   * One agent in the rail: avatar, name, role badge and the last thing it said.
   *
   * AVATAR SWAP POINT — the only place in `/llm` that draws an agent avatar.
   * It delegates to `$components/omni/RailAvatar.svelte` (from `f5-rail-avatar`):
   * Omni in a disc tinted by the agent's skin, with the selected and speaking
   * rings. Changing the avatar anywhere in the section means changing this file
   * and nothing else.
   */

  import RailAvatar from "$components/omni/RailAvatar.svelte";
  import { t } from "$lib/i18n";
  import { agentTint, roleLabelKey, roleText, type AgentDef } from "$lib/llm/types";

  let {
    agent,
    active = false,
    speaking = false,
    preview = "",
    onselect,
  }: {
    agent: AgentDef;
    active?: boolean;
    speaking?: boolean;
    preview?: string;
    onselect: (id: string) => void;
  } = $props();

  let tint = $derived(agentTint(agent));
  let roleLabel = $derived(roleText(agent.role) ?? $t(roleLabelKey(agent.role)));
</script>

<button
  type="button"
  class="rail-item"
  class:active
  aria-current={active ? "true" : undefined}
  onclick={() => onselect(agent.id)}
>
  <!-- AVATAR SWAP POINT (see the comment at the top of this file). -->
  <RailAvatar {tint} size={36} {active} {speaking} />
  <span class="rail-item-body">
    <span class="rail-item-head">
      <span class="rail-item-name">{agent.name}</span>
      <span class="tag rail-item-role">{roleLabel}</span>
    </span>
    <span class="rail-item-preview">{preview}</span>
  </span>
</button>

<style>
  .rail-item {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    width: 100%;
    height: 56px;
    padding: 0 var(--space-2);
    border: none;
    border-radius: var(--radius-md);
    background: transparent;
    color: var(--text);
    text-align: left;
    cursor: pointer;
    transition: background var(--duration-fast) var(--ease-out);
  }

  @media (hover: hover) {
    .rail-item:hover {
      background: var(--fill-2);
    }
  }

  .rail-item.active {
    background: var(--accent-soft);
  }

  .rail-item-body {
    display: flex;
    flex-direction: column;
    gap: 1px;
    min-width: 0;
    flex: 1;
  }

  .rail-item-head {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    min-width: 0;
  }

  .rail-item-name {
    font-size: var(--text-sm);
    font-weight: 600;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .rail-item-role {
    flex-shrink: 0;
    font-size: var(--text-caption);
    text-transform: lowercase;
  }

  .rail-item-preview {
    font-size: var(--text-caption);
    color: var(--text-dim);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
</style>
