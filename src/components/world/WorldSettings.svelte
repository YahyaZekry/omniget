<script lang="ts">
  /**
   * Settings → World: the measured tier, the pinned one, a recalibration and
   * the one destructive action the world has (delete it).
   *
   * The renderer is imported here only when "recalibrate" is clicked, so
   * opening Settings costs nothing.
   */
  import { onMount } from "svelte";
  import { getVersion } from "@tauri-apps/api/app";
  import { t } from "$lib/i18n";
  import { getSettings, updateSettings } from "$lib/stores/settings-store.svelte";
  import { showToast } from "$lib/stores/toast-store.svelte";

  type Tier = 0 | 1 | 2 | 3;
  const TIERS: Tier[] = [0, 1, 2, 3];

  let settings = $derived(getSettings());
  let enabled = $derived(settings?.world?.enabled ?? true);
  let measured = $derived(settings?.world?.tier_measured ?? null);
  let medianMs = $derived(settings?.world?.measured_median_ms ?? null);
  let measuredVersion = $derived(settings?.world?.measured_app_version ?? null);
  let pinned = $derived(settings?.world?.tier_override ?? null);

  let calibrating = $state(false);
  let lowered = $state<Tier | null>(null);
  let exists = $state<boolean | null>(null);
  let confirmingDelete = $state(false);
  let deleting = $state(false);

  function tierLabel(tier: number | null): string {
    if (tier === null || tier === undefined) return $t("world.settings.tier_none") as string;
    return `${tier} · ${$t(`world.settings.tier_names.${tier}`)}`;
  }

  async function recalibrate() {
    if (calibrating) return;
    calibrating = true;
    try {
      // The one place in Settings that loads the renderer, and only on click.
      const render = await import("$lib/world/render");
      const detail = await render.calibrateDetailed(null);
      let version: string | null = null;
      try {
        version = await getVersion();
      } catch {
        version = null;
      }
      await updateSettings({
        world: {
          tier_measured: detail.tier,
          measured_median_ms: Number.isFinite(detail.medianMs) ? detail.medianMs : null,
          measured_app_version: version,
        },
      });
      lowered = null;
    } catch (error) {
      showToast("error", `${$t("world.error")} ${String(error)}`);
    } finally {
      calibrating = false;
    }
  }

  async function pin(value: string) {
    const next = value === "auto" ? null : (Number(value) as Tier);
    await updateSettings({ world: { tier_override: next } });
  }

  async function toggleEnabled() {
    const next = !enabled;
    await updateSettings({ world: { enabled: next } });
    if (!next) {
      // Switching the world off has to stop it, not hide it: with the setting
      // already false, `world_close` shuts the tick thread down instead of
      // dropping it to 0.2 Hz.
      try {
        const { invoke } = await import("@tauri-apps/api/core");
        await invoke("world_close");
      } catch {
        // Nothing was running, which is the outcome we wanted anyway.
      }
    }
  }

  async function deleteWorld() {
    if (!confirmingDelete) {
      confirmingDelete = true;
      return;
    }
    deleting = true;
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      await invoke("world_delete");
      exists = false;
      showToast("success", $t("world.settings.deleted") as string);
    } catch (error) {
      showToast("error", String(error));
    } finally {
      deleting = false;
      confirmingDelete = false;
    }
  }

  onMount(() => {
    void (async () => {
      try {
        const { invoke } = await import("@tauri-apps/api/core");
        exists = (await invoke("world_exists")) as boolean;
      } catch {
        // The bridge may not be in this build yet: the section still works.
        exists = null;
      }
    })();
    // Emitted by the renderer's watchdog while /world is open in this webview.
    const onLowered = (event: Event) => {
      const to = (event as CustomEvent<{ to?: Tier }>).detail?.to;
      lowered = typeof to === "number" ? to : 0;
    };
    window.addEventListener("world:tier-changed", onLowered);
    return () => window.removeEventListener("world:tier-changed", onLowered);
  });
</script>

<section class="settings-section" id="settings-world">
  <h5 class="section-title">{$t("world.settings.title")}</h5>
  <p class="section-desc">{$t("world.settings.desc")}</p>

  <div class="card">
    <div class="setting-row">
      <div class="setting-col">
        <span class="setting-label">{$t("world.settings.enabled")}</span>
        <span class="setting-path">{$t("world.settings.enabled_desc")}</span>
      </div>
      <button
        class="toggle"
        class:on={enabled}
        onclick={toggleEnabled}
        role="switch"
        aria-checked={enabled}
        aria-label={$t("world.settings.enabled") as string}
      ><span class="toggle-knob"></span></button>
    </div>

    <div class="divider"></div>

    <div class="setting-row">
      <div class="setting-col">
        <span class="setting-label">{$t("world.settings.thinking")}</span>
        <span class="setting-path">{$t("world.settings.thinking_desc")}</span>
      </div>
      <button
        class="toggle"
        class:on={getSettings()?.world?.thinking ?? false}
        disabled={!enabled}
        onclick={() => updateSettings({ world: { thinking: !(getSettings()?.world?.thinking ?? false) } })}
        role="switch"
        aria-checked={getSettings()?.world?.thinking ?? false}
        aria-label={$t("world.settings.thinking") as string}
      ><span class="toggle-knob"></span></button>
    </div>

    <div class="divider"></div>

    <div class="setting-row">
      <div class="setting-col">
        <span class="setting-label">{$t("world.settings.room_server")}</span>
        <span class="setting-path">{$t("world.settings.room_server_desc")}</span>
      </div>
      <input
        class="room-server"
        type="text"
        placeholder="wss://…/v1/room"
        spellcheck="false"
        autocapitalize="off"
        disabled={!enabled}
        value={getSettings()?.world?.room_server ?? ""}
        onchange={(e) => updateSettings({ world: { room_server: (e.currentTarget as HTMLInputElement).value.trim() } })}
        aria-label={$t("world.settings.room_server") as string}
      />
    </div>

    <div class="divider"></div>

    <div class="setting-row">
      <div class="setting-col">
        <span class="setting-label">{$t("world.settings.tier_measured")}</span>
        <span class="setting-path">
          {#if measured === null}
            {$t("world.settings.never_measured")}
          {:else}
            {tierLabel(measured)}{#if medianMs !== null} · {medianMs.toFixed(1)} ms{/if}{#if measuredVersion} · v{measuredVersion}{/if}
          {/if}
        </span>
      </div>
      <button class="btn btn-secondary" onclick={recalibrate} disabled={calibrating}>
        {calibrating ? $t("world.settings.recalibrating") : $t("world.settings.recalibrate")}
      </button>
    </div>

    <div class="divider"></div>

    <div class="setting-row">
      <div class="setting-col">
        <span class="setting-label">{$t("world.settings.tier_pinned")}</span>
        <span class="setting-path">{$t("world.settings.tier_pinned_desc")}</span>
      </div>
      <select class="input-text select" value={pinned === null ? "auto" : String(pinned)} onchange={(e) => pin((e.target as HTMLSelectElement).value)}>
        <option value="auto">{$t("world.settings.tier_auto")}</option>
        {#each TIERS as tier (tier)}
          <option value={String(tier)}>{tierLabel(tier)}</option>
        {/each}
      </select>
    </div>

    {#if lowered !== null}
      <div class="divider"></div>
      <div class="setting-row">
        <div class="setting-col">
          <span class="setting-label">{$t("world.settings.tier_lowered")}</span>
          <span class="setting-path">{tierLabel(lowered)}</span>
        </div>
      </div>
    {/if}

    <div class="divider"></div>

    <div class="setting-row">
      <div class="setting-col">
        <span class="setting-label">{$t("world.settings.delete")}</span>
        <span class="setting-path">
          {enabled && exists === false
            ? $t("world.settings.delete_none")
            : $t("world.settings.delete_desc")}
        </span>
      </div>
      <!-- With the world switched off `world_exists` answers "no" by design, so
           the button stays available: "I turned it off, now erase it" has to
           work. Deleting nothing is not an error. -->
      <button
        class="btn btn-secondary"
        onclick={deleteWorld}
        disabled={deleting || (enabled && exists === false)}
      >
        {confirmingDelete ? $t("world.settings.delete_confirm") : $t("world.settings.delete")}
      </button>
    </div>
  </div>
</section>

<style>
  .room-server {
    width: 16rem;
    padding: 5px 9px;
    border-radius: 8px;
    border: 1px solid var(--separator, rgba(127, 127, 127, 0.3));
    background: var(--fill-quaternary, rgba(127, 127, 127, 0.08));
    color: var(--text);
    font: inherit;
    font-size: var(--text-sm);
    font-family: var(--font-mono, ui-monospace, monospace);
  }

  .section-desc {
    margin: 0 0 12px;
    color: var(--text-secondary);
    font-size: 13px;
  }
</style>
