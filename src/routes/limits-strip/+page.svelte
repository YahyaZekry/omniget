<script lang="ts">
  /**
   * The limits strip: the whole content of the `limits-strip` window.
   *
   * The window is built, sized and parked in Rust (`limits_strip::commands`);
   * it is exactly as big as what is drawn here, because a transparent window
   * still swallows clicks. Collapsed it is the pill; a click on a ring asks
   * Rust to grow the window and the card appears next to the pill.
   *
   * At rest this page owns no timer: it redraws when `limits://state`
   * arrives. The only clock is a 30 s one for "resets in…", alive while a
   * card is open.
   */
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { listen, type UnlistenFn } from "@tauri-apps/api/event";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { t } from "$lib/i18n";

  type Edge = "top" | "right" | "bottom" | "left";
  type Activity = "idle" | "working" | "waiting" | "done";
  type LimitWindow = {
    id: string;
    label: string;
    used: number | null;
    used_abs: number | null;
    limit_abs: number | null;
    unit: string | null;
    resets_at: number | null;
    group: string | null;
  };
  type LocalModel = {
    name: string;
    size_bytes: number | null;
    vram_bytes: number | null;
    context: number | null;
    quant: string | null;
    params: string | null;
    expires_at: number | null;
  };
  type Reading = {
    windows: LimitWindow[];
    plan: string | null;
    account: string | null;
    note: string | null;
    local_models: LocalModel[];
  };
  type Ring = {
    id: string;
    label: string;
    local: boolean;
    beta: boolean;
    status: "pending" | "ok" | "absent" | "needs_auth" | "rate_limited" | "error";
    message: string | null;
    reading: Reading | null;
    read_at: number | null;
    activity: Activity;
  };
  type Snapshot = { open: boolean; edge: Edge; rings: Ring[] };

  /** Ring geometry: a 28 px SVG, the stroke centred on r = 11. */
  const R = 11;
  const CIRC = 2 * Math.PI * R;

  let snap = $state<Snapshot>({ open: true, edge: "top", rings: [] });
  let openId = $state<string | null>(null);
  let offset = $state(0);
  let now = $state(Date.now());
  let pulsing = $state<string | null>(null);
  let solid = $state(false);

  let vertical = $derived(snap.edge === "left" || snap.edge === "right");
  let openRing = $derived(snap.rings.find((r) => r.id === openId) ?? null);

  /** The window that decides the ring: the fullest one. */
  function worst(ring: Ring): number | null {
    const used = (ring.reading?.windows ?? []).map((w) => w.used).filter((u): u is number => u != null);
    return used.length ? Math.max(...used) : null;
  }

  function tone(used: number | null): "calm" | "amber" | "red" {
    if (used == null || used < 0.8) return "calm";
    return used < 0.95 ? "amber" : "red";
  }

  function percent(used: number): number {
    return Math.round(used * 100);
  }

  function dash(used: number | null): string {
    const f = Math.max(0, Math.min(1, used ?? 0));
    return `${f * CIRC} ${CIRC}`;
  }

  function span(ms: number): string {
    const minutes = Math.max(1, Math.round(ms / 60000));
    const d = Math.floor(minutes / 1440);
    const h = Math.floor((minutes % 1440) / 60);
    const m = minutes % 60;
    const parts: string[] = [];
    if (d) parts.push(`${d} ${$t("llm.limits.strip.d")}`);
    if (h) parts.push(`${h} ${$t("llm.limits.strip.h")}`);
    if (m && !d) parts.push(`${m} ${$t("llm.limits.strip.min")}`);
    return parts.join(" ");
  }

  function resets(at: number | null): string | null {
    if (at == null) return null;
    return at <= now
      ? ($t("llm.limits.strip.resets_now") as string)
      : ($t("llm.limits.strip.resets_in", { time: span(at - now) }) as string);
  }

  function amount(n: number, unit: string | null): string {
    const digits = unit === "usd" ? 2 : 0;
    return n.toLocaleString(undefined, { maximumFractionDigits: digits, minimumFractionDigits: digits });
  }

  function absolute(w: LimitWindow): string | null {
    if (w.used_abs == null) return null;
    const unit = w.unit ?? "";
    return w.limit_abs != null
      ? ($t("llm.limits.strip.of", { used: amount(w.used_abs, w.unit), limit: amount(w.limit_abs, w.unit), unit }) as string)
      : ($t("llm.limits.strip.count", { used: amount(w.used_abs, w.unit), unit }) as string);
  }

  function gib(bytes: number | null): string | null {
    return bytes ? `${(bytes / 1073741824).toFixed(1)} GB` : null;
  }

  function modelLine(m: LocalModel): string {
    const left =
      m.expires_at && m.expires_at > now
        ? ($t("llm.limits.strip.unloads_in", { time: span(m.expires_at - now) }) as string)
        : null;
    return [m.params, m.quant, gib(m.vram_bytes ?? m.size_bytes), left].filter(Boolean).join(" · ");
  }

  function tooltip(ring: Ring): string {
    const used = worst(ring);
    const head = used != null ? `${ring.label} · ${$t("llm.limits.strip.used", { percent: percent(used) })}` : ring.label;
    return ring.activity === "idle" ? head : `${head} · ${$t(`llm.limits.strip.state.${ring.activity}`)}`;
  }

  function readAgo(ring: Ring): string | null {
    if (ring.read_at == null) return null;
    const ms = now - ring.read_at;
    return ms < 60000
      ? ($t("llm.limits.strip.just_now") as string)
      : ($t("llm.limits.strip.read_ago", { time: span(ms) }) as string);
  }

  async function expand(id: string | null) {
    try {
      const res = (await invoke("limits_strip_set_expanded", { expanded: id !== null })) as { offset: number };
      offset = res.offset;
    } catch {
      offset = 0;
    }
    now = Date.now();
    openId = id;
  }

  function toggle(id: string) {
    void expand(openId === id ? null : id);
  }

  function refresh(id: string) {
    void invoke("limits_strip_refresh", { providerId: id }).catch(() => {});
  }

  function hide() {
    void invoke("limits_strip_close").catch(() => {});
  }

  function grab(event: MouseEvent) {
    if (event.button !== 0 || openId) return;
    void getCurrentWindow().startDragging().catch(() => {});
  }

  // The 30 s clock lives only while a card shows a countdown.
  $effect(() => {
    if (!openId) return;
    const timer = setInterval(() => (now = Date.now()), 30000);
    return () => clearInterval(timer);
  });

  // The ring whose card is open was switched off: fold the window back.
  $effect(() => {
    if (openId && !snap.rings.some((r) => r.id === openId)) void expand(null);
  });

  onMount(() => {
    solid = new URLSearchParams(location.search).has("solid");
    const root = document.documentElement;
    const prev = [root.style.background, document.body.style.background];
    if (!solid) {
      root.style.background = "transparent";
      document.body.style.background = "transparent";
    }

    const unlisteners: UnlistenFn[] = [];
    let disposed = false;
    const arm = (p: Promise<UnlistenFn>) =>
      void p.then((un) => (disposed ? un() : unlisteners.push(un))).catch(() => {});

    arm(listen<Snapshot>("limits://state", (e) => (snap = e.payload)));
    arm(
      listen<{ provider: string }>("limits://chime", (e) => {
        pulsing = e.payload.provider;
        setTimeout(() => (pulsing = null), 2400);
      }),
    );
    void invoke<Snapshot>("limits_strip_state")
      .then((s) => (snap = s))
      .catch(() => {});

    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && openId) void expand(null);
    };
    const onBlur = () => {
      if (openId) void expand(null);
    };
    window.addEventListener("keydown", onKey);
    window.addEventListener("blur", onBlur);
    return () => {
      disposed = true;
      unlisteners.forEach((un) => un());
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("blur", onBlur);
      [root.style.background, document.body.style.background] = prev;
    };
  });
</script>

<main class="stage edge-{snap.edge}" class:vertical class:solid oncontextmenu={(e) => e.preventDefault()}>
  <div
    class="pill"
    class:vertical
    role="toolbar"
    aria-label={$t("llm.limits.strip.aria") as string}
    style={vertical ? `margin-top:${offset}px` : `margin-left:${offset}px`}
  >
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <span class="grip" onmousedown={grab} aria-hidden="true"><i></i><i></i><i></i></span>
    {#each snap.rings as ring (ring.id)}
      {@const used = worst(ring)}
      <button
        type="button"
        class="ring tone-{tone(used)}"
        class:open={openId === ring.id}
        class:pulse={pulsing === ring.id}
        class:dim={ring.status !== "ok" && !ring.reading}
        title={tooltip(ring)}
        aria-label={tooltip(ring)}
        aria-expanded={openId === ring.id}
        onclick={() => toggle(ring.id)}
      >
        <svg viewBox="0 0 28 28" width="28" height="28" aria-hidden="true">
          <circle class="track" cx="14" cy="14" r={R} />
          {#if used != null}
            <circle class="arc" cx="14" cy="14" r={R} stroke-dasharray={dash(used)} transform="rotate(-90 14 14)" />
          {/if}
          <text x="14" y="14" class="glyph">
            {ring.local ? (ring.reading?.local_models.length ?? 0) : ring.label.slice(0, 2)}
          </text>
        </svg>
        {#if ring.activity !== "idle"}
          <span class="dot dot-{ring.activity}"></span>
        {:else if ring.status === "needs_auth" || ring.status === "error" || ring.status === "rate_limited"}
          <span class="dot dot-problem"></span>
        {/if}
      </button>
    {:else}
      <span class="empty" title={$t("llm.limits.strip.empty") as string}>–</span>
    {/each}
  </div>

  {#if openRing}
    {@const ring = openRing}
    <section class="card" aria-label={ring.label}>
      <header class="card-head">
        <span class="card-title">{ring.label}</span>
        {#if ring.beta}<span class="tag" title={$t("llm.limits.beta_hint") as string}>{$t("llm.limits.beta")}</span>{/if}
        {#if ring.local}<span class="tag">{$t("llm.limits.local")}</span>{/if}
        {#if ring.activity !== "idle"}
          <span class="state state-{ring.activity}">{$t(`llm.limits.strip.state.${ring.activity}`)}</span>
        {/if}
      </header>

      <div class="card-body">
        {#if ring.status !== "ok"}
          <p class="status">
            {$t(`llm.limits.strip.status.${ring.status}`)}
            {#if ring.message}<span class="status-detail">{ring.message}</span>{/if}
          </p>
        {/if}

        {#if ring.reading}
          {#each ring.reading.windows as w (w.id)}
            {@const abs = absolute(w)}
            {@const when = resets(w.resets_at)}
            <div class="win tone-{tone(w.used)}">
              <div class="win-top">
                <span class="win-label">{w.group ? `${w.group} · ${w.label}` : w.label}</span>
                {#if w.used != null}<span class="win-pct">{percent(w.used)}%</span>{/if}
              </div>
              <div class="bar"><span style={`width:${Math.min(100, percent(w.used ?? 0))}%`}></span></div>
              {#if abs || when}
                <div class="win-sub">{[abs, when].filter(Boolean).join(" · ")}</div>
              {/if}
            </div>
          {/each}

          {#if ring.reading.local_models.length}
            <h3 class="sub">{$t("llm.limits.strip.models")}</h3>
            {#each ring.reading.local_models as m (m.name)}
              <div class="model">
                <span class="model-name">{m.name}</span>
                <span class="win-sub">{modelLine(m)}</span>
              </div>
            {/each}
          {/if}

          {#if ring.reading.plan}
            <p class="meta">{$t("llm.limits.strip.plan")}: {ring.reading.plan}</p>
          {/if}
          {#if ring.reading.account}<p class="meta">{ring.reading.account}</p>{/if}
          {#if ring.reading.note}<p class="meta">{ring.reading.note}</p>{/if}
        {/if}
      </div>

      <footer class="card-foot">
        <span class="meta">{readAgo(ring) ?? ""}</span>
        <span class="foot-actions">
          {#if ring.id !== "omniget"}
            <button type="button" class="link" onclick={() => refresh(ring.id)}>{$t("llm.limits.strip.refresh")}</button>
          {/if}
          <button type="button" class="link" onclick={hide}>{$t("llm.limits.strip.hide")}</button>
        </span>
      </footer>
    </section>
  {/if}
</main>

<style>
  :global(html),
  :global(body) {
    margin: 0;
    overflow: hidden;
  }

  .stage {
    --calm: #64d2ff;
    --amber: #ff9f0a;
    --red: #ff453a;
    --ink: rgba(255, 255, 255, 0.92);
    --ink-dim: rgba(255, 255, 255, 0.58);
    --glass: rgba(28, 28, 30, 0.94);
    position: fixed;
    inset: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
    align-items: flex-start;
    color: var(--ink);
    font: 12px/1.35 -apple-system, BlinkMacSystemFont, "Segoe UI", system-ui, sans-serif;
    user-select: none;
    -webkit-user-select: none;
    cursor: default;
  }
  /* No alpha channel to trust: a square backdrop instead of see-through corners. */
  .stage.solid {
    background: #1c1c1e;
  }
  .stage.edge-bottom {
    flex-direction: column-reverse;
  }
  .stage.edge-left {
    flex-direction: row;
  }
  .stage.edge-right {
    flex-direction: row-reverse;
  }

  .pill {
    flex: none;
    box-sizing: border-box;
    height: 44px;
    padding: 0 8px;
    display: flex;
    align-items: center;
    border-radius: 14px;
    background: var(--glass);
    box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.08);
  }
  .pill.vertical {
    height: auto;
    width: 44px;
    padding: 8px 0;
    flex-direction: column;
  }

  .grip {
    flex: none;
    width: 14px;
    height: 28px;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 3px;
    cursor: grab;
  }
  .pill.vertical .grip {
    width: 28px;
    height: 14px;
    flex-direction: row;
  }
  .grip i {
    width: 3px;
    height: 3px;
    border-radius: 50%;
    background: rgba(255, 255, 255, 0.35);
  }

  .ring {
    flex: none;
    position: relative;
    width: 36px;
    height: 36px;
    padding: 4px;
    border: none;
    border-radius: 10px;
    background: transparent;
    color: inherit;
    cursor: pointer;
  }
  .ring:hover,
  .ring.open {
    background: rgba(255, 255, 255, 0.1);
  }
  .ring:focus-visible {
    outline: 2px solid var(--calm);
    outline-offset: -2px;
  }
  .ring.dim {
    opacity: 0.5;
  }
  .ring svg {
    display: block;
  }
  .track {
    fill: none;
    stroke: rgba(255, 255, 255, 0.16);
    stroke-width: 3.5;
  }
  .arc {
    fill: none;
    stroke: var(--tone);
    stroke-width: 3.5;
    stroke-linecap: round;
    transition: stroke-dasharray 400ms ease;
  }
  .glyph {
    fill: var(--ink);
    font-size: 8.5px;
    font-weight: 700;
    text-anchor: middle;
    dominant-baseline: central;
  }
  .tone-calm {
    --tone: var(--calm);
  }
  .tone-amber {
    --tone: var(--amber);
  }
  .tone-red {
    --tone: var(--red);
  }
  .ring.pulse {
    animation: pulse 0.8s ease-in-out 3;
  }
  @keyframes pulse {
    50% {
      background: color-mix(in srgb, var(--tone) 40%, transparent);
    }
  }

  .dot {
    position: absolute;
    right: 3px;
    bottom: 3px;
    width: 8px;
    height: 8px;
    border-radius: 50%;
    box-shadow: 0 0 0 2px #1c1c1e;
  }
  .dot-working {
    background: #0a84ff;
    animation: breathe 1.6s ease-in-out infinite;
  }
  .dot-waiting {
    background: var(--amber);
  }
  .dot-done {
    background: #30d158;
  }
  .dot-problem {
    background: rgba(255, 255, 255, 0.45);
  }
  @keyframes breathe {
    50% {
      opacity: 0.35;
    }
  }

  .empty {
    width: 36px;
    text-align: center;
    color: var(--ink-dim);
  }

  .card {
    flex: 1;
    align-self: stretch;
    min-width: 0;
    min-height: 0;
    box-sizing: border-box;
    display: flex;
    flex-direction: column;
    border-radius: 14px;
    background: var(--glass);
    box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.08);
    user-select: text;
    -webkit-user-select: text;
  }
  .card-head {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 10px 12px 6px;
  }
  .card-title {
    font-size: 13px;
    font-weight: 700;
  }
  .tag {
    font-size: 9px;
    font-weight: 700;
    letter-spacing: 0.05em;
    text-transform: uppercase;
    padding: 1px 5px;
    border-radius: 999px;
    background: rgba(255, 255, 255, 0.12);
    color: var(--ink-dim);
  }
  .state {
    margin-left: auto;
    font-size: 11px;
    color: var(--ink-dim);
  }
  .state-working {
    color: #0a84ff;
  }
  .state-waiting {
    color: var(--amber);
  }
  .state-done {
    color: #30d158;
  }
  .card-body {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 0 12px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .status {
    margin: 0;
    color: var(--amber);
  }
  .status-detail {
    display: block;
    color: var(--ink-dim);
  }
  .win-top {
    display: flex;
    justify-content: space-between;
    gap: 8px;
  }
  .win-label {
    font-weight: 600;
  }
  .win-pct {
    font-variant-numeric: tabular-nums;
    color: var(--tone);
    font-weight: 700;
  }
  .bar {
    height: 4px;
    margin: 3px 0;
    border-radius: 2px;
    background: rgba(255, 255, 255, 0.14);
    overflow: hidden;
  }
  .bar span {
    display: block;
    height: 100%;
    border-radius: 2px;
    background: var(--tone);
  }
  .win-sub,
  .meta {
    margin: 0;
    font-size: 11px;
    color: var(--ink-dim);
  }
  .sub {
    margin: 2px 0 0;
    font-size: 11px;
    font-weight: 700;
    color: var(--ink-dim);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .model {
    display: flex;
    flex-direction: column;
  }
  .model-name {
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .card-foot {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 6px 12px 8px;
  }
  .foot-actions {
    display: flex;
    gap: 10px;
  }
  .link {
    border: none;
    padding: 0;
    background: none;
    font: inherit;
    font-size: 11px;
    color: var(--calm);
    cursor: pointer;
  }
  .link:hover {
    text-decoration: underline;
  }

  @media (prefers-reduced-motion: reduce) {
    .arc {
      transition: none;
    }
    .ring.pulse,
    .dot-working {
      animation: none;
    }
  }
</style>
