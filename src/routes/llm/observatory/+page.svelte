<script lang="ts">
  /**
   * Observatory — the instrument panel of the LLM section.
   *
   * Top to bottom: quota as energy (one `QuotaMeter` per account, in router
   * order, with the active candidate marked), cost per day, one card per agent
   * (throughput, first token, tokens, cache, cost, Context Guardian, timeline),
   * the wire probe, and the diagnostics list.
   *
   * Cadence: the page creates no timer. It pulls one snapshot on mount and
   * then rides `llm://telemetry`, which the backend pushes at 2 Hz only while
   * a turn is alive. Leaving the route unsubscribes; a closed section costs
   * zero.
   */
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { page } from "$app/stores";
  import { t } from "$lib/i18n";
  import { showToast } from "$lib/stores/toast-store.svelte";
  import QuotaMeter from "$components/llm/QuotaMeter.svelte";
  import AgentTelemetryCard from "$components/llm/observatory/AgentTelemetryCard.svelte";
  import CostByDay from "$components/llm/observatory/CostByDay.svelte";
  import DiagnosticsPanel from "$components/llm/observatory/DiagnosticsPanel.svelte";
  import WireProbePanel from "$components/llm/observatory/WireProbePanel.svelte";
  import {
    getTelemetry,
    getTelemetryError,
    getTelemetryState,
    isDemo,
    loadCostFallback,
    loadTelemetry,
    resetAgentHistory,
    startTelemetry,
    WINDOWS,
    type QuotaStatus,
    type WindowKey,
  } from "$lib/stores/llm-telemetry-store.svelte";

  const WINDOW_KEYS = Object.keys(WINDOWS) as WindowKey[];

  let windowKey = $state<WindowKey>("24h");
  let busy = $state(false);

  let snapshot = $derived(getTelemetry());
  let dataState = $derived(getTelemetryState());
  let errorCode = $derived(getTelemetryError());
  let demo = $derived(isDemo());
  /** Clock of the data, not of the wall: reading it costs no timer. */
  let nowMs = $derived(snapshot.ts_ms || Date.now());

  /** Accounts in router order, so the fallback chain reads top to bottom. */
  let chain = $derived.by<QuotaStatus[]>(() => {
    const byId = new Map(snapshot.quotas.map((q) => [q.id, q]));
    const ordered = snapshot.fallback_chain.map((id) => byId.get(id)).filter((q): q is QuotaStatus => !!q);
    const rest = snapshot.quotas.filter((q) => !snapshot.fallback_chain.includes(q.id));
    return [...ordered, ...rest];
  });

  function projection(q: QuotaStatus): string | undefined {
    if (!q.exhausts_at_ms) return undefined;
    const mins = Math.max(0, Math.round((q.exhausts_at_ms - nowMs) / 60_000));
    const h = Math.floor(mins / 60);
    const left = h > 0 ? `${h} h ${mins % 60} min` : `${mins} min`;
    return `${$t("llm.observatory.quota_projection")} ${left}`;
  }

  async function refresh() {
    busy = true;
    try {
      await loadTelemetry();
      await loadCostFallback();
    } finally {
      busy = false;
    }
  }

  async function cancelTurn(requestId: string) {
    try {
      await invoke("llm_turn_cancel", { requestId });
      await loadTelemetry();
    } catch (e) {
      showToast("error", String(e));
    }
  }

  onMount(() => {
    let stop: (() => void) | null = null;
    const demoFlag = $page.url.searchParams.get("demo") === "1";
    startTelemetry({ demo: demoFlag }).then((fn) => {
      stop = fn;
      if (!demoFlag) void loadCostFallback();
    });
    return () => stop?.();
  });
</script>

<div class="observatory">
  <header class="page-head">
    <div>
      <h1 class="page-title">{$t("llm.observatory.title")}</h1>
      <p class="page-lede">{$t("llm.observatory.lede")}</p>
    </div>
    <div class="segmented">
      {#each WINDOW_KEYS as key (key)}
        <button class="segmented-btn" class:active={windowKey === key} type="button" onclick={() => (windowKey = key)}>
          {$t(`llm.observatory.window_${key}`)}
        </button>
      {/each}
    </div>
  </header>

  {#if dataState === "unavailable" && snapshot.agents.length === 0 && !demo}
    <div class="empty-state">
      <div class="empty-state-title">{$t("llm.observatory.empty_title")}</div>
      <div class="empty-state-hint">{$t("llm.observatory.empty_hint")} <code>{errorCode}</code></div>
      <button class="btn btn-secondary" type="button" disabled={busy} onclick={refresh}>
        {$t("llm.observatory.refresh")}
      </button>
    </div>
  {:else}
    {#if chain.length}
      <section>
        <span class="group-label">{$t("llm.observatory.quota")}</span>
        <div class="card quotas">
          {#each chain as q, i (q.id)}
            <div class="quota-row" class:active={q.active}>
              <span class="rank" aria-hidden="true">{i + 1}</span>
              <QuotaMeter
                label={q.label}
                used={q.used}
                resetsAt={q.resets_at_ms ?? undefined}
                projection={projection(q)}
                active={q.active}
              />
              {#if q.source === "estimated"}
                <span class="src-tag">{$t("llm.observatory.source_estimated")}</span>
              {/if}
            </div>
          {/each}
        </div>
      </section>
    {/if}

    {#if snapshot.cost_by_day.length}
      <section>
        <span class="group-label">{$t("llm.observatory.cost_by_day")}</span>
        <div class="card chart-card">
          <CostByDay days={snapshot.cost_by_day} />
        </div>
      </section>
    {/if}

    <section>
      <span class="group-label">{$t("llm.observatory.agents")}</span>
      {#if snapshot.agents.length}
        <div class="agents">
          {#each snapshot.agents as agent (agent.agent_id)}
            <AgentTelemetryCard {agent} {windowKey} {nowMs} onReset={resetAgentHistory} />
          {/each}
        </div>
      {:else}
        <div class="card"><div class="setting-row">{$t("llm.observatory.no_agents")}</div></div>
      {/if}
    </section>

    <section>
      <!-- No group label here: WireProbePanel already renders its own title and
           description, and two "Wire probe" headings stacked read as a bug. -->
      <WireProbePanel />
    </section>

    <section>
      <span class="group-label">{$t("llm.observatory.diagnostics")}</span>
      <DiagnosticsPanel
        agents={snapshot.agents}
        state={dataState}
        {errorCode}
        tsMs={snapshot.ts_ms}
        {demo}
        {busy}
        onRefresh={refresh}
        onCancel={cancelTurn}
        onReset={resetAgentHistory}
      />
    </section>
  {/if}
</div>

<style>
  .observatory {
    display: flex;
    flex-direction: column;
    gap: var(--space-6);
    padding: var(--space-5);
    max-width: 1100px;
  }
  .page-head {
    display: flex;
    align-items: flex-end;
    gap: var(--space-4);
  }
  .page-head .segmented {
    margin-left: auto;
  }
  .segmented-btn.active {
    background: var(--surface-hi);
    color: var(--text);
  }
  section {
    display: flex;
    flex-direction: column;
  }
  .quotas {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
    padding: var(--space-4);
  }
  .quota-row {
    display: grid;
    grid-template-columns: 18px 1fr auto;
    align-items: center;
    gap: var(--space-3);
  }
  .rank {
    font-size: var(--text-caption);
    color: var(--text-dim);
    font-variant-numeric: tabular-nums;
    text-align: center;
  }
  .quota-row.active .rank {
    color: var(--accent);
  }
  .src-tag {
    font-size: var(--text-caption);
    color: var(--text-dim);
  }
  .chart-card {
    padding: var(--space-4);
  }
  .agents {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(340px, 1fr));
    gap: var(--space-4);
  }
  code {
    font-family: var(--font-mono);
    font-size: var(--text-caption);
  }
</style>
