<script lang="ts">
  /** Placeholder for the local tab; it is built in a later phase. */
  import { t } from "$lib/i18n";
</script>

<svelte:head><title>{$t("llm.tab.local")}</title></svelte:head>

<div class="page soon">
  <h1 class="page-title">{$t("llm.tab.local")}</h1>
  <p class="page-lede">{$t("llm.soon_title")} — {$t("llm.soon_body")}</p>
</div>

<style>
  .soon {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
  }
</style>
