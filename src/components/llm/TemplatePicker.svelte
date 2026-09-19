<script lang="ts">
  /**
   * Team templates. The shapes live on the Rust side
   * (`llm_roster_apply_template`); this is the list plus one Apply button.
   */
  import { t } from "$lib/i18n";

  let {
    onapply,
    disabled = false,
  }: { onapply: (template: string) => void; disabled?: boolean } = $props();

  const TEMPLATES = [
    { id: "solo", title: "llm.template.solo", desc: "llm.template.solo_desc" },
    { id: "duo", title: "llm.template.duo", desc: "llm.template.duo_desc" },
    { id: "research", title: "llm.template.research", desc: "llm.template.research_desc" },
  ];
</script>

<div class="group">
  <div class="group-label">{$t("llm.roster.templates")}</div>
  {#each TEMPLATES as template (template.id)}
    <div class="group-row">
      <div class="group-row-content">
        <div class="group-row-title">{$t(template.title)}</div>
        <div class="group-row-sub">{$t(template.desc)}</div>
      </div>
      <div class="group-row-trailing">
        <button type="button" class="button" {disabled} onclick={() => onapply(template.id)}>
          {$t("llm.roster.apply")}
        </button>
      </div>
    </div>
  {/each}
</div>
