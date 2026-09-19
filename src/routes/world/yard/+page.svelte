<script lang="ts">
  import WorldCanvas from '$components/world/WorldCanvas.svelte';
  import { t } from '$lib/i18n';
</script>

<svelte:head><title>{$t('world.yard.title')} · OmniGet</title></svelte:head>

<section class="yard-page">
  <header>
    <a href="/world">← {$t('world.title')}</a>
    <h1>{$t('world.yard.title')}</h1>
    <p>{$t('world.yard.subtitle')}</p>
    <small>{$t('world.yard.hint')}</small>
  </header>
  <WorldCanvas previewMap="/world/house-yard-v1.json" />
</section>

<style>
  .yard-page { padding: 1.5rem; display: grid; gap: 1.25rem; }
  header { border-left: 3px solid #bc965d; padding-left: 1rem; }
  a { color: #bc965d; font-size: .8rem; text-decoration: none; }
  h1 { margin: .65rem 0 .3rem; font-size: 1.6rem; }
  p { margin: 0 0 .5rem; opacity: .8; }
  small { opacity: .65; }
</style>
