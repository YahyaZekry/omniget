<script lang="ts">
  // Any CLI that speaks the Agent Client Protocol becomes an agent of the
  // roster. One read on mount (a PATH lookup), nothing polls.
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { t } from "$lib/i18n";
  import { showToast } from "$lib/stores/toast-store.svelte";
  import { loadRoster } from "$lib/stores/llm-store.svelte";

  type AcpCli = { id: string; name: string; command: string; args: string[]; path: string | null; installed: boolean };
  let clis = $state<AcpCli[]>([]);
  let busy = $state<string | null>(null);
  let customName = $state("");
  let customCommand = $state("");

  onMount(async () => {
    try {
      clis = (await invoke("llm_acp_detect")) as AcpCli[];
    } catch {
      clis = [];
    }
  });

  async function add(name: string, command: string, args: string[]) {
    busy = command;
    try {
      await invoke("llm_acp_agent_create", { name, command, args });
      await loadRoster();
      showToast("success", $t("llm.accounts.acp_added", { name }) as string);
    } catch (error) {
      showToast("error", String(error));
    } finally {
      busy = null;
    }
  }

  function addCustom() {
    const parts = customCommand.trim().split(/\s+/).filter(Boolean);
    if (parts.length === 0) return;
    void add(customName.trim() || parts[0], parts[0], parts.slice(1));
    customName = "";
    customCommand = "";
  }
</script>

<section class="surface-card acp">
  <h2 class="section-title">{$t("llm.accounts.acp_title")}</h2>
  <p class="field-hint">{$t("llm.accounts.acp_hint")}</p>
  <ul class="acp-list">
    {#each clis as cli (cli.id)}
      <li class="acp-row" class:missing={!cli.installed}>
        <div class="acp-text">
          <span class="acp-name">{cli.name}</span>
          <code class="acp-cmd">{cli.path ?? `${cli.command} ${cli.args.join(" ")}`}</code>
        </div>
        {#if cli.installed}
          <button type="button" class="button" disabled={busy === cli.command} onclick={() => add(cli.name, cli.command, cli.args)}>
            {$t("llm.accounts.acp_add")}
          </button>
        {:else}
          <span class="acp-state">{$t("llm.accounts.acp_missing")}</span>
        {/if}
      </li>
    {/each}
  </ul>
  <form class="acp-custom" onsubmit={(e) => { e.preventDefault(); addCustom(); }}>
    <input type="text" bind:value={customName} placeholder={$t("llm.accounts.acp_custom_name") as string} />
    <input type="text" class="grow" bind:value={customCommand} placeholder="my-agent --acp" spellcheck="false" autocapitalize="off" />
    <button type="submit" class="button" disabled={!customCommand.trim()}>{$t("llm.accounts.acp_add")}</button>
  </form>
</section>

<style>
  .acp {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    margin-top: var(--space-4);
  }
  .acp-list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .acp-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-3);
  }
  .acp-row.missing {
    opacity: 0.55;
  }
  .acp-text {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .acp-name {
    font-weight: 600;
  }
  .acp-cmd {
    font-size: var(--text-xs, 11px);
    color: var(--text-dim);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .acp-state {
    font-size: var(--text-sm);
    color: var(--text-dim);
  }
  .acp-custom {
    display: flex;
    gap: var(--space-2);
  }
  .acp-custom input {
    padding: 6px 10px;
    border-radius: var(--radius-md, 8px);
    border: 1px solid var(--separator, rgba(127, 127, 127, 0.3));
    background: var(--fill-quaternary, rgba(127, 127, 127, 0.08));
    color: var(--text);
    font: inherit;
    font-size: var(--text-sm);
    min-width: 0;
  }
  .acp-custom .grow {
    flex: 1;
    font-family: var(--font-mono, ui-monospace, monospace);
  }
</style>
