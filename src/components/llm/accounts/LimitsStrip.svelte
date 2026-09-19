<script lang="ts">
  // Settings of the limits strip. Off by default: nothing is read and no
  // window exists until the master switch is on, and every reader has its own
  // box. One read of the prefs on mount, one `limits://prefs` listener (the
  // strip can be dragged to another edge), nothing polls.
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { t } from "$lib/i18n";
  import { showToast } from "$lib/stores/toast-store.svelte";

  type Edge = "top" | "right" | "bottom" | "left";
  type ProviderPref = { id: string; enabled: boolean; muted: boolean };
  type Prefs = {
    enabled: boolean;
    edge: Edge;
    along: Record<string, number>;
    providers: ProviderPref[];
    notify_thresholds: boolean;
    thresholds: number[];
    notify_reset: boolean;
  };
  type ProviderInfo = { id: string; label: string; local: boolean; beta: boolean; detected: boolean };
  type Described = { prefs: Prefs; open: boolean; providers: ProviderInfo[] };

  const EDGES: Edge[] = ["top", "right", "bottom", "left"];

  let prefs = $state<Prefs | null>(null);
  let infos = $state<ProviderInfo[]>([]);
  let busy = $state(false);
  let unavailable = $state(false);

  function take(described: Described) {
    prefs = described.prefs;
    infos = described.providers;
  }

  onMount(() => {
    let off: (() => void) | undefined;
    let gone = false;
    void (async () => {
      try {
        take((await invoke("limits_strip_get_prefs")) as Described);
      } catch {
        unavailable = true;
        return;
      }
      const un = await listen<Prefs>("limits://prefs", (event) => {
        prefs = event.payload;
      });
      if (gone) un();
      else off = un;
    })();
    return () => {
      gone = true;
      off?.();
    };
  });

  async function save(next: Prefs) {
    if (busy) return;
    busy = true;
    const before = prefs;
    prefs = next;
    try {
      take((await invoke("limits_strip_set_prefs", { prefs: next })) as Described);
    } catch (error) {
      prefs = before;
      showToast("error", String(error));
    } finally {
      busy = false;
    }
  }

  function toggleMaster() {
    if (prefs) void save({ ...prefs, enabled: !prefs.enabled });
  }

  function setEdge(edge: Edge) {
    if (prefs && prefs.edge !== edge) void save({ ...prefs, edge });
  }

  function toggleProvider(id: string) {
    if (!prefs) return;
    void save({
      ...prefs,
      providers: prefs.providers.map((p) => (p.id === id ? { ...p, enabled: !p.enabled } : p)),
    });
  }

  function info(id: string): ProviderInfo | undefined {
    return infos.find((i) => i.id === id);
  }
</script>

<section class="surface-card limits">
  <h2 class="section-title">{$t("llm.limits.title")}</h2>
  <p class="field-hint">{$t("llm.limits.hint")}</p>

  {#if unavailable}
    <p class="field-hint">{$t("llm.limits.unavailable")}</p>
  {:else if prefs}
    <div class="limits-row">
      <span class="limits-name">{$t("llm.limits.enable")}</span>
      <button
        type="button"
        role="switch"
        class="switch"
        class:on={prefs.enabled}
        aria-checked={prefs.enabled}
        aria-label={$t("llm.limits.enable") as string}
        disabled={busy}
        onclick={toggleMaster}
      >
        <span class="knob"></span>
      </button>
    </div>

    <div class="limits-row">
      <span class="limits-name">{$t("llm.limits.edge")}</span>
      <div class="segmented" role="radiogroup" aria-label={$t("llm.limits.edge") as string}>
        {#each EDGES as edge (edge)}
          <button
            type="button"
            role="radio"
            class="segment"
            class:active={prefs.edge === edge}
            aria-checked={prefs.edge === edge}
            disabled={busy}
            onclick={() => setEdge(edge)}
          >
            {$t(`llm.limits.edge_${edge}`)}
          </button>
        {/each}
      </div>
    </div>
    <p class="field-hint">{$t("llm.limits.drag_hint")}</p>

    <h3 class="limits-sub">{$t("llm.limits.providers")}</h3>
    <p class="field-hint">{$t("llm.limits.privacy")}</p>
    <ul class="limits-list">
      {#each prefs.providers as p (p.id)}
        {@const meta = info(p.id)}
        <li class="limits-provider" class:missing={meta && !meta.detected}>
          <label class="limits-check">
            <input type="checkbox" checked={p.enabled} disabled={busy} onchange={() => toggleProvider(p.id)} />
            <span class="limits-text">
              <span class="limits-name">
                {meta?.label ?? p.id}
                {#if meta?.local}<span class="tag">{$t("llm.limits.local")}</span>{/if}
                {#if meta?.beta}<span class="tag beta" title={$t("llm.limits.beta_hint") as string}>{$t("llm.limits.beta")}</span>{/if}
                {#if meta && !meta.detected}<span class="limits-dim">{$t("llm.limits.not_found")}</span>{/if}
              </span>
              <span class="limits-reads">{$t(`llm.limits.reads.${p.id}`)}</span>
            </span>
          </label>
        </li>
      {/each}
    </ul>
  {/if}
</section>

<style>
  .limits {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    margin-top: var(--space-4);
  }
  .limits-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-3);
  }
  .limits-name {
    font-weight: 600;
    display: inline-flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 6px;
  }
  .limits-sub {
    margin: var(--space-2) 0 0;
    font-size: var(--text-sm);
    font-weight: 600;
  }
  .limits-list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .limits-provider.missing {
    opacity: 0.6;
  }
  .limits-check {
    display: flex;
    align-items: flex-start;
    gap: var(--space-2);
    cursor: pointer;
  }
  .limits-check input {
    margin-top: 3px;
    accent-color: var(--accent);
  }
  .limits-text {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .limits-reads,
  .limits-dim {
    font-size: var(--text-xs, 11px);
    font-weight: 400;
    color: var(--text-dim);
  }
  .tag {
    font-size: 10px;
    font-weight: 600;
    padding: 1px 6px;
    border-radius: 999px;
    background: var(--fill-quaternary, rgba(127, 127, 127, 0.14));
    color: var(--text-dim);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .tag.beta {
    background: color-mix(in srgb, #ff9f0a 22%, transparent);
    color: color-mix(in srgb, #ff9f0a 80%, var(--text));
  }
  .switch {
    width: 38px;
    height: 22px;
    flex: none;
    border-radius: 12px;
    border: none;
    padding: 0;
    position: relative;
    cursor: pointer;
    background: var(--fill-secondary, rgba(127, 127, 127, 0.32));
    transition: background 150ms;
  }
  .switch.on {
    background: var(--accent);
  }
  .switch:disabled {
    opacity: 0.6;
  }
  .knob {
    position: absolute;
    top: 2px;
    left: 2px;
    width: 18px;
    height: 18px;
    border-radius: 50%;
    background: #fff;
    box-shadow: 0 1px 3px rgba(0, 0, 0, 0.25);
    transition: transform 150ms;
  }
  .switch.on .knob {
    transform: translateX(16px);
  }
  .segmented {
    display: inline-flex;
    padding: 2px;
    border-radius: var(--radius-md, 8px);
    background: var(--fill-quaternary, rgba(127, 127, 127, 0.12));
  }
  .segment {
    border: none;
    background: transparent;
    color: var(--text);
    font: inherit;
    font-size: var(--text-sm);
    padding: 4px 10px;
    border-radius: 6px;
    cursor: pointer;
  }
  .segment.active {
    background: var(--accent);
    color: var(--accent-text, #fff);
  }
  @media (prefers-reduced-motion: reduce) {
    .switch,
    .knob {
      transition: none;
    }
  }
</style>
