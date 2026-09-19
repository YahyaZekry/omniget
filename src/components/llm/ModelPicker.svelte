<script lang="ts">
  /**
   * Provider + model picker.
   *
   * Providers come from the AI-keys table (`tool_keys_kinds`) and the keys the
   * user already saved (`tool_keys_list`); the model list is only fetched when
   * the user clicks Load, never on mount — `llm_models_list` hits the provider.
   * While that command is a stub the model id stays a free-text field, so the
   * picker is usable either way.
   */
  import { onMount, untrack } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { t } from "$lib/i18n";
  import type { ModelRef } from "$lib/llm/types";

  let {
    value = null,
    onchange,
  }: {
    value?: ModelRef | null;
    onchange: (ref: ModelRef) => void;
  } = $props();

  type Kind = { id: string; name: string };
  type KeyView = { id: string; kind: string; name: string };

  let kinds = $state<Kind[]>([]);
  let saved = $state<KeyView[]>([]);
  // Seeded once: after mount the two fields are the source of truth and every
  // change is pushed back through `onchange`.
  let provider = $state(untrack(() => value?.provider ?? ""));
  let model = $state(untrack(() => value?.model ?? ""));
  let models = $state<string[]>([]);
  let loading = $state(false);
  let loadError = $state(false);

  async function loadProviders() {
    try {
      kinds = (await invoke<Kind[] | null>("tool_keys_kinds")) ?? [];
    } catch {
      kinds = [];
    }
    try {
      saved = (await invoke<KeyView[] | null>("tool_keys_list")) ?? [];
    } catch {
      saved = [];
    }
    if (!provider) provider = saved[0]?.kind ?? kinds[0]?.id ?? "";
  }

  async function loadModels() {
    if (!provider) return;
    loading = true;
    loadError = false;
    try {
      const list = await invoke<string[] | null>("llm_models_list", { provider });
      models = Array.isArray(list) ? list : [];
      loadError = models.length === 0;
    } catch {
      models = [];
      loadError = true;
    } finally {
      loading = false;
    }
  }

  function commit() {
    if (!provider || !model.trim()) return;
    onchange({ provider, model: model.trim() });
  }

  let hasKey = $derived(saved.some((k) => k.kind === provider));

  onMount(() => {
    void loadProviders();
  });
</script>

<div class="picker">
  <label class="field">
    <span class="field-label">{$t("llm.roster.provider")}</span>
    <select class="input" bind:value={provider} onchange={() => { models = []; commit(); }}>
      {#each kinds as kind (kind.id)}
        <option value={kind.id}>{kind.name}</option>
      {/each}
    </select>
    {#if !hasKey && provider}
      <span class="field-hint">{$t("llm.models.add_key_hint")}</span>
    {/if}
  </label>

  <label class="field">
    <span class="field-label">{$t("llm.roster.model")}</span>
    <input class="input" bind:value={model} list="llm-model-options" onchange={commit} />
    <datalist id="llm-model-options">
      {#each models as m (m)}
        <option value={m}></option>
      {/each}
    </datalist>
  </label>

  <div class="picker-actions">
    <button type="button" class="button" onclick={loadModels} disabled={loading || !provider}>
      {loading ? $t("llm.models.loading") : $t("llm.models.load")}
    </button>
    {#if models.length > 0}
      <span class="picker-note">{$t("llm.models.count", { n: models.length })}</span>
    {:else if loadError}
      <span class="picker-note">{$t("llm.err.unavailable")}</span>
    {/if}
  </div>
</div>

<style>
  .picker {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
  }

  .picker-actions {
    display: flex;
    align-items: center;
    gap: var(--space-3);
  }

  .picker-note {
    font-size: var(--text-caption);
    color: var(--text-dim);
  }
</style>
