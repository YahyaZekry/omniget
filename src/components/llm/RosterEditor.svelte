<script lang="ts">
  /**
   * Editor of one agent: name, role, system prompt, model policy (Fixed or a
   * Route chain), granted tools and budget. Tools are read-only until the MCP
   * phase ships a catalogue; the list a saved agent already carries is shown so
   * nothing is silently dropped on save.
   */
  import { onMount, untrack } from "svelte";
  import { t } from "$lib/i18n";
  import type { AgentDef, AgentRole, Candidate, ModelRef } from "$lib/llm/types";
  import { getSkills, loadSkills } from "$lib/stores/llm-skills-store.svelte";
  import ModelPicker from "./ModelPicker.svelte";

  let {
    agent,
    onsave,
    oncancel,
    ondelete,
  }: {
    agent: AgentDef;
    onsave: (agent: AgentDef) => void;
    oncancel: () => void;
    ondelete?: (id: string) => void;
  } = $props();

  const ROLES = ["coordinator", "worker", "advisor"] as const;

  function cloneAgent(source: AgentDef): AgentDef {
    const copy = structuredClone($state.snapshot(source)) as AgentDef;
    copy.budget ??= {};
    copy.tools ??= [];
    copy.skills ??= [];
    return copy;
  }

  // Seeded once from the prop on purpose: the form owns its draft and the
  // parent re-keys the component when it hands over another agent.
  let draft = $state<AgentDef>(untrack(() => cloneAgent(agent)));
  let roleValue = $state(untrack(() => (typeof agent.role === "string" ? agent.role : "custom")));
  let customRole = $state(untrack(() => (typeof agent.role === "string" ? "" : agent.role.custom)));

  let isRoute = $derived(draft.model.policy === "route");

  onMount(() => {
    void loadSkills();
  });

  function toggleSkill(name: string, on: boolean) {
    const current = draft.skills ?? [];
    draft.skills = on ? [...current, name] : current.filter((s) => s !== name);
  }

  function setPolicy(next: "fixed" | "route") {
    if (next === draft.model.policy) return;
    draft.model =
      next === "fixed"
        ? { policy: "fixed", model: { provider: "openai", model: "" } }
        : { policy: "route", chain: [] };
  }

  function setFixedModel(ref: ModelRef) {
    draft.model = { policy: "fixed", model: ref };
  }

  function addCandidate() {
    if (draft.model.policy !== "route") return;
    const candidate: Candidate = {
      runtime: "native",
      provider: "openrouter",
      model: "",
      max_cost_per_1k: null,
      min_context: 0,
    };
    draft.model = { policy: "route", chain: [...draft.model.chain, candidate] };
  }

  function removeCandidate(index: number) {
    if (draft.model.policy !== "route") return;
    draft.model = { policy: "route", chain: draft.model.chain.filter((_, i) => i !== index) };
  }

  function role(): AgentRole {
    return roleValue === "custom" ? { custom: customRole.trim() || "custom" } : (roleValue as AgentRole);
  }

  function save() {
    onsave({ ...($state.snapshot(draft) as AgentDef), role: role() });
  }
</script>

<form class="editor" onsubmit={(e) => { e.preventDefault(); save(); }}>
  <label class="field">
    <span class="field-label">{$t("llm.roster.name")}</span>
    <input class="input" bind:value={draft.name} required maxlength="48" />
  </label>

  <label class="field">
    <span class="field-label">{$t("llm.roster.role")}</span>
    <select class="input" bind:value={roleValue}>
      {#each ROLES as r (r)}
        <option value={r}>{$t(`llm.role.${r}`)}</option>
      {/each}
      <option value="custom">{$t("llm.role.custom")}</option>
    </select>
  </label>

  {#if roleValue === "custom"}
    <label class="field">
      <span class="field-label">{$t("llm.role.custom")}</span>
      <input class="input" bind:value={customRole} maxlength="32" />
    </label>
  {/if}

  <label class="field">
    <span class="field-label">{$t("llm.roster.system_prompt")}</span>
    <textarea class="input prompt" rows="4" bind:value={draft.system_prompt}></textarea>
  </label>

  <fieldset class="group-block">
    <legend class="field-label">{$t("llm.roster.model_policy")}</legend>
    <div class="mac-segmented" role="tablist">
      <button
        type="button"
        class="mac-segmented-btn"
        role="tab"
        aria-selected={!isRoute}
        onclick={() => setPolicy("fixed")}
      >
        {$t("llm.roster.fixed")}
      </button>
      <button
        type="button"
        class="mac-segmented-btn"
        role="tab"
        aria-selected={isRoute}
        onclick={() => setPolicy("route")}
      >
        {$t("llm.roster.route")}
      </button>
    </div>

    {#if draft.model.policy === "fixed"}
      <ModelPicker value={draft.model.model} onchange={setFixedModel} />
    {:else}
      <ol class="chain">
        {#each draft.model.chain as candidate, index (index)}
          <li class="chain-row">
            <input
              class="input"
              bind:value={candidate.model}
              placeholder={$t("llm.roster.model")}
              aria-label={$t("llm.roster.model")}
            />
            <button type="button" class="button" onclick={() => removeCandidate(index)}>
              {$t("llm.roster.remove")}
            </button>
          </li>
        {/each}
      </ol>
      <button type="button" class="button" onclick={addCandidate}>
        {$t("llm.roster.add_candidate")}
      </button>
    {/if}
  </fieldset>

  <fieldset class="group-block">
    <legend class="field-label">{$t("llm.roster.tools")}</legend>
    {#if (draft.tools ?? []).length === 0}
      <p class="hint">
        {$t("llm.roster.tools_empty")}
        <a class="mcp-link" href="/llm/mcp">{$t("llm.mcp.grants")}</a>
      </p>
    {:else}
      <ul class="tools">
        {#each draft.tools ?? [] as grant, index (index)}
          <li>
            <span class="mono">
              {grant.source === "mcp" ? `${grant.server}/${grant.tool}` : grant.name}
            </span>
            <select class="input mode" bind:value={grant.mode}>
              <option value="auto">{$t("llm.inspector.grant_auto")}</option>
              <option value="ask">{$t("llm.inspector.grant_ask")}</option>
              <option value="deny">{$t("llm.inspector.grant_deny")}</option>
            </select>
          </li>
        {/each}
      </ul>
      <a class="mcp-link" href="/llm/mcp">{$t("llm.mcp.grants")}</a>
    {/if}
  </fieldset>

  <fieldset class="group-block">
    <legend class="field-label">{$t("llm.roster.skills")}</legend>
    {#if getSkills().length === 0}
      <p class="hint">{$t("llm.roster.skills_empty")}</p>
    {:else}
      <ul class="tools">
        {#each getSkills() as skill (skill.name)}
          <li>
            <label class="skill-row">
              <input
                type="checkbox"
                checked={(draft.skills ?? []).includes(skill.name)}
                onchange={(e) => toggleSkill(skill.name, e.currentTarget.checked)}
              />
              <span class="mono">{skill.name}</span>
            </label>
          </li>
        {/each}
      </ul>
    {/if}
  </fieldset>

  <fieldset class="group-block">
    <legend class="field-label">{$t("llm.roster.budget")}</legend>
    <div class="budget">
      <label class="field">
        <span class="field-label">{$t("llm.inspector.usd_per_day")}</span>
        <input class="input" type="number" min="0" step="0.1" bind:value={draft.budget!.usd_per_day} />
      </label>
      <label class="field">
        <span class="field-label">{$t("llm.inspector.tokens_per_turn")}</span>
        <input class="input" type="number" min="0" step="1000" bind:value={draft.budget!.tokens_per_turn} />
      </label>
      <label class="field">
        <span class="field-label">{$t("llm.inspector.max_tool_calls")}</span>
        <input class="input" type="number" min="0" max="255" bind:value={draft.budget!.max_tool_calls_per_turn} />
      </label>
    </div>
  </fieldset>

  <div class="actions">
    <button type="submit" class="button primary">{$t("llm.roster.save")}</button>
    <button type="button" class="button" onclick={oncancel}>{$t("llm.roster.cancel")}</button>
    {#if ondelete}
      <button type="button" class="button danger" onclick={() => ondelete?.(draft.id)}>
        {$t("llm.roster.delete")}
      </button>
    {/if}
  </div>
</form>

<style>
  .editor {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
    max-width: 560px;
  }

  .group-block {
    border: var(--hairline) solid var(--separator);
    border-radius: var(--radius-md);
    padding: var(--space-3);
    margin: 0;
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
  }

  .prompt {
    resize: vertical;
    line-height: var(--leading-base);
  }

  .chain {
    list-style: decimal;
    margin: 0;
    padding-left: var(--space-5);
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }

  .chain-row {
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }

  .tools {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }

  .tools li {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-2);
  }

  .mode {
    width: 120px;
  }

  .mono {
    font-family: var(--font-mono);
    font-size: var(--text-sm);
  }

  .skill-row {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    cursor: pointer;
  }

  /* The full grant table lives in /llm/mcp; this is the way there. */
  .mcp-link {
    font-size: var(--text-sm);
    color: var(--accent-hi);
    text-decoration: none;
    align-self: flex-start;
  }

  .hint {
    margin: 0;
    font-size: var(--text-sm);
    color: var(--text-dim);
  }

  .budget {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(150px, 1fr));
    gap: var(--space-3);
  }

  .actions {
    display: flex;
    gap: var(--space-2);
  }
</style>
