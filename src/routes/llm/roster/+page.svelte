<script lang="ts">
  /** Roster tab: the agent list, the editor and the team templates. */
  import { onMount } from "svelte";
  import { t } from "$lib/i18n";
  import type { AgentDef } from "$lib/llm/types";
  import { agentTint, roleLabelKey, roleText, modelLabel } from "$lib/llm/types";
  import {
    applyTemplate,
    deleteAgent,
    getAgents,
    isRosterAvailable,
    loadRoster,
    saveAgent,
  } from "$lib/stores/llm-store.svelte";
  import { tintToCss } from "$lib/stores/profile-store.svelte";
  import RosterEditor from "$components/llm/RosterEditor.svelte";
  import TemplatePicker from "$components/llm/TemplatePicker.svelte";

  let agents = $derived(getAgents());
  let editing = $state<AgentDef | null>(null);

  onMount(() => {
    void loadRoster();
  });

  function blankAgent(): AgentDef {
    return {
      id: `agent-${Date.now().toString(36)}`,
      name: "",
      role: "worker",
      system_prompt: "",
      model: { policy: "fixed", model: { provider: "openai", model: "" } },
      tools: [],
      skills: [],
      budget: {},
      runtime: { kind: "native" },
      skin: { id: "omni-default", tint: [110, 139, 255] },
    };
  }

  function onSave(agent: AgentDef) {
    void saveAgent(agent);
    editing = null;
  }
</script>

<svelte:head><title>{$t("llm.roster.title")}</title></svelte:head>

<div class="page page-wide roster-page">
  <header class="page-head">
    <div>
      <h1 class="page-title">{$t("llm.roster.title")}</h1>
      <p class="page-lede">{$t("llm.subtitle")}</p>
    </div>
    <button type="button" class="button primary" onclick={() => (editing = blankAgent())}>
      {$t("llm.roster.new")}
    </button>
  </header>

  {#if !isRosterAvailable()}
    <p class="notice" role="status">{$t("llm.roster.unavailable")}</p>
  {/if}

  {#if editing}
    {#key editing.id}
    <RosterEditor
      agent={editing}
      onsave={onSave}
      oncancel={() => (editing = null)}
      ondelete={agents.some((a) => a.id === editing?.id)
        ? (id) => {
            void deleteAgent(id);
            editing = null;
          }
        : undefined}
    />
    {/key}
  {:else}
    <div class="group">
      {#each agents as agent (agent.id)}
        <button type="button" class="group-row agent-row" onclick={() => (editing = agent)}>
          <span class="dot" style:background={tintToCss(agentTint(agent))}></span>
          <span class="group-row-content">
            <span class="group-row-title">{agent.name}</span>
            <span class="group-row-sub">
              {roleText(agent.role) ?? $t(roleLabelKey(agent.role))} · {modelLabel(agent.model)}
            </span>
          </span>
          <span class="group-row-trailing chevron">›</span>
        </button>
      {/each}
    </div>

    <TemplatePicker onapply={(id) => void applyTemplate(id)} />
  {/if}
</div>

<style>
  /* Block layout on purpose: as a flex column the groups would shrink to fit
     the viewport and clip their own rows instead of letting the page scroll. */
  .roster-page {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
  }

  .roster-page :global(.group) {
    margin-bottom: var(--space-5);
  }

  .notice {
    margin: 0 0 var(--space-4);
    font-size: var(--text-sm);
    color: var(--text-dim);
  }

  .agent-row {
    width: 100%;
    border: none;
    background: transparent;
    text-align: left;
    cursor: pointer;
    color: inherit;
    display: flex;
    align-items: center;
    gap: var(--space-3);
  }

  .dot {
    width: 24px;
    height: 24px;
    border-radius: var(--radius-full);
    flex-shrink: 0;
  }
</style>
