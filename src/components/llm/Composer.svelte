<script lang="ts">
  /**
   * Composer: one auto-growing textarea, Enter sends, Shift+Enter breaks the
   * line. While a turn runs the primary action becomes Stop, so there is only
   * ever one primary action on screen.
   */
  import { t } from "$lib/i18n";

  let {
    disabled = false,
    running = false,
    onsend,
    onstop,
  }: {
    disabled?: boolean;
    running?: boolean;
    onsend: (text: string) => void;
    onstop: () => void;
  } = $props();

  const MAX_HEIGHT = 200;

  let value = $state("");
  let textarea = $state<HTMLTextAreaElement | null>(null);

  function autosize() {
    const el = textarea;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, MAX_HEIGHT)}px`;
  }

  function send() {
    const text = value.trim();
    if (!text || disabled || running) return;
    value = "";
    queueMicrotask(autosize);
    onsend(text);
  }

  function onKeydown(event: KeyboardEvent) {
    if (event.key === "Enter" && !event.shiftKey && !event.isComposing) {
      event.preventDefault();
      send();
    }
  }
</script>

<div class="composer">
  <textarea
    bind:this={textarea}
    bind:value
    class="input composer-input"
    rows="1"
    {disabled}
    placeholder={$t("llm.composer.placeholder")}
    aria-label={$t("llm.composer.placeholder")}
    oninput={autosize}
    onkeydown={onKeydown}
  ></textarea>
  <div class="composer-actions">
    <span class="composer-hint">{$t("llm.composer.hint")}</span>
    {#if running}
      <button type="button" class="button composer-send" onclick={onstop}>
        {$t("llm.conv.stop")}
      </button>
    {:else}
      <button
        type="button"
        class="button primary composer-send"
        disabled={disabled || value.trim().length === 0}
        onclick={send}
      >
        {$t("llm.composer.send")}
      </button>
    {/if}
  </div>
</div>

<style>
  .composer {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    padding: var(--space-3) var(--space-5) var(--space-4);
    border-top: var(--hairline) solid var(--separator);
  }

  .composer-input {
    width: 100%;
    resize: none;
    min-height: 40px;
    max-height: 200px;
    line-height: var(--leading-base);
    padding: var(--space-2) var(--space-3);
  }

  .composer-actions {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-3);
  }

  .composer-hint {
    font-size: var(--text-caption);
    color: var(--text-dim);
  }

  .composer-send {
    min-width: 96px;
  }
</style>
