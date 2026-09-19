<script lang="ts">
  /**
   * Default routing rule for new agents. The rule only orders the candidate
   * chain a `ModelPolicy::Route` carries; the coordinator does the walking.
   * It is stored in `localStorage` because it is a UI default, not state the
   * backend owns — once `llm_roster_*` is wired this moves into the roster
   * file (noted in the handoff as an open issue).
   */
  import { onMount } from "svelte";
  import { t } from "$lib/i18n";

  const KEY = "omniget.llm.routing_rule";
  const RULES = [
    { id: "cheap", title: "llm.models.rule_cheap", desc: "llm.models.rule_cheap_desc" },
    { id: "local", title: "llm.models.rule_local", desc: "llm.models.rule_local_desc" },
    { id: "quality", title: "llm.models.rule_quality", desc: "llm.models.rule_quality_desc" },
  ];

  let rule = $state("cheap");

  function pick(id: string) {
    rule = id;
    try {
      localStorage.setItem(KEY, id);
    } catch {
      // Private mode or a blocked store: the rule just does not persist.
    }
  }

  onMount(() => {
    try {
      const saved = localStorage.getItem(KEY);
      if (saved && RULES.some((r) => r.id === saved)) rule = saved;
    } catch {
      // Same as above.
    }
  });
</script>

<div class="group rules-group">
  <div class="group-label">{$t("llm.models.routing")}</div>
  {#each RULES as item (item.id)}
    <button
      type="button"
      class="group-row rule"
      aria-pressed={rule === item.id}
      onclick={() => pick(item.id)}
    >
      <span class="group-row-content">
        <span class="group-row-title">{$t(item.title)}</span>
        <span class="group-row-sub">{$t(item.desc)}</span>
      </span>
      <span class="group-row-trailing">
        {#if rule === item.id}<span class="tag tag-accent">✓</span>{/if}
      </span>
    </button>
  {/each}
  <div class="group-footer">{$t("llm.models.rule_hint")}</div>
</div>

<style>
  /* The selected row paints a full-bleed background, so the group has to clip
     it or the highlight squares off the rounded corners. */
  .rules-group {
    overflow: hidden;
  }

  .rule {
    width: 100%;
    border: none;
    background: transparent;
    text-align: left;
    cursor: pointer;
    color: inherit;
  }

  .rule[aria-pressed="true"] {
    background: var(--accent-soft);
  }
</style>
