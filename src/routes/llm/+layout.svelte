<script lang="ts">
  /**
   * Shell of the LLM section: the tab bar plus the active page. Each tab is a
   * SvelteKit route, so its code only loads when the user opens it — the chat
   * shell never pulls the Observatory or the Roster editor into its chunk.
   */
  import type { Snippet } from "svelte";
  import { page } from "$app/state";
  import { t } from "$lib/i18n";

  let { children }: { children: Snippet } = $props();

  const TABS = [
    { href: "/llm", key: "llm.tab.chat" },
    { href: "/llm/jobs", key: "llm.tab.jobs" },
    { href: "/llm/loops", key: "llm.tab.loops" },
    { href: "/llm/roster", key: "llm.tab.roster" },
    { href: "/llm/models", key: "llm.tab.models" },
    { href: "/llm/observatory", key: "llm.tab.observatory" },
    { href: "/llm/mcp", key: "llm.tab.mcp" },
    { href: "/llm/skills", key: "llm.tab.skills" },
    { href: "/llm/accounts", key: "llm.tab.accounts" },
    { href: "/llm/local", key: "llm.tab.local" },
    { href: "/llm/arena", key: "llm.tab.arena" },
  ];

  let path = $derived(page.url.pathname.replace(/\/$/, "") || "/llm");
</script>

<div class="llm-root">
  <nav class="llm-tabs" aria-label={$t("llm.title")}>
    {#each TABS as tab (tab.href)}
      <a
        class="llm-tab"
        href={tab.href}
        aria-current={path === tab.href ? "page" : undefined}
      >
        {$t(tab.key)}
      </a>
    {/each}
  </nav>
  <div class="llm-body">
    {@render children()}
  </div>
</div>

<style>
  .llm-root {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .llm-tabs {
    display: flex;
    align-items: center;
    gap: 2px;
    padding: var(--space-2) var(--space-3);
    border-bottom: var(--hairline) solid var(--separator);
    overflow-x: auto;
    flex-shrink: 0;
  }

  .llm-tab {
    padding: 4px var(--space-3);
    border-radius: var(--radius-sm);
    font-size: var(--text-sm);
    font-weight: 500;
    color: var(--text-muted);
    text-decoration: none;
    white-space: nowrap;
  }

  .llm-tab[aria-current="page"] {
    background: var(--accent-soft);
    color: var(--accent-hi);
  }

  .llm-body {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }
</style>
