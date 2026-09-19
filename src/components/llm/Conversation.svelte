<script lang="ts">
  import WorkspaceChip from "./WorkspaceChip.svelte";
  /**
   * Centre column: header (agent, effective model, switch-model, new chat),
   * the messages and the composer. The streaming answer is a bubble built from
   * the live turn, so it follows the same layout as a committed message.
   */
  import { tick } from "svelte";
  import { t } from "$lib/i18n";
  import type { AgentDef, ChatMessage, ModelRef } from "$lib/llm/types";
  import { effectiveModelLabel } from "$lib/llm/types";
  import {
    cancelTurn,
    getActiveConversation,
    getActiveTurn,
    getMessages,
    getToolAsk,
    answerTool,
    newConversation,
    sendMessage,
    switchModel,
  } from "$lib/stores/llm-store.svelte";
  import Composer from "./Composer.svelte";
  import MessageBubble from "./MessageBubble.svelte";
  import ModelPicker from "./ModelPicker.svelte";

  let { agent = null }: { agent?: AgentDef | null } = $props();

  let scroller = $state<HTMLDivElement | null>(null);
  let pickerOpen = $state(false);

  let conversation = $derived(getActiveConversation());
  let messages = $derived(getMessages());
  let turn = $derived(getActiveTurn());
  let ask = $derived(getToolAsk());
  let modelText = $derived(effectiveModelLabel(conversation, agent));

  let streamingMessage = $derived<ChatMessage | null>(
    turn
      ? {
          id: `turn-${turn.requestId}`,
          role: "assistant",
          text: turn.text,
          toolCalls: turn.toolCalls.length > 0 ? turn.toolCalls : undefined,
          thinking: turn.thinking || undefined,
          modelLabel: modelText,
          error: turn.error,
        }
      : null,
  );

  async function stickToBottom() {
    await tick();
    const el = scroller;
    if (el) el.scrollTop = el.scrollHeight;
  }

  // A permission request must never be born below the fold, and a streaming
  // turn keeps the newest line in view while the reader is already at the end.
  $effect(() => {
    if (ask) void stickToBottom();
  });
  $effect(() => {
    void turn?.text;
    void turn?.toolCalls.length;
    const el = scroller;
    if (el && el.scrollHeight - el.scrollTop - el.clientHeight < 160) void stickToBottom();
  });

  function onSend(text: string) {
    void sendMessage(text).then(stickToBottom);
    void stickToBottom();
  }

  function onSwitch(ref: ModelRef) {
    void switchModel(ref);
    pickerOpen = false;
  }

  // Follow the stream without a timer: the text length is the only trigger.
  $effect(() => {
    turn?.text.length;
    messages.length;
    void stickToBottom();
  });
</script>

<section class="conv">
  <header class="conv-head">
    <div class="conv-title">
      <h2>{agent?.name ?? $t("llm.conv.no_agent")}</h2>
      {#if modelText}<span class="conv-model">{modelText}</span>{/if}
      <WorkspaceChip conversationId={conversation?.id ?? null} refreshKey={turn ? 1 : 0} />
    </div>
    <div class="conv-actions">
      <button
        type="button"
        class="button"
        onclick={() => (pickerOpen = !pickerOpen)}
        aria-expanded={pickerOpen}
        disabled={!conversation}
      >
        {$t("llm.conv.switch_model")}
      </button>
      <button
        type="button"
        class="button"
        disabled={!agent}
        onclick={() => agent && newConversation(agent.id)}
      >
        {$t("llm.conv.new")}
      </button>
    </div>
  </header>

  {#if pickerOpen}
    <div class="conv-picker">
      <ModelPicker value={conversation?.model ?? null} onchange={onSwitch} />
    </div>
  {/if}

  <div class="conv-scroll" bind:this={scroller}>
    {#if messages.length === 0 && !streamingMessage}
      <div class="empty-state">
        <p class="empty-state-title">{$t("llm.conv.empty_title")}</p>
        <p class="empty-state-body">{$t("llm.conv.empty_body")}</p>
      </div>
    {:else}
      {#each messages as message (message.id)}
        <MessageBubble {message} {agent} />
      {/each}
      {#if streamingMessage}
        <MessageBubble message={streamingMessage} {agent} streaming />
      {/if}
    {/if}

    {#if ask}
      <div class="tool-ask" role="alertdialog" aria-labelledby="llm-tool-ask-title">
        <div class="tool-ask-text">
          <span id="llm-tool-ask-title" class="tool-ask-title">{$t("llm.conv.tool_ask")}</span>
          <span class="tool-ask-tool">{ask.tool}</span>
          {#if ask.preview}<pre class="tool-ask-preview">{ask.preview}</pre>{/if}
        </div>
        <div class="tool-ask-actions">
          <button
            type="button"
            class="button primary"
            onclick={() => void answerTool(ask!.tool_call_id, true)}
          >
            {$t("llm.conv.allow")}
          </button>
          {#if ask.tool !== "external_directory"}
            <button
              type="button"
              class="button"
              title={$t("llm.conv.always_hint") as string}
              onclick={() => void answerTool(ask!.tool_call_id, true, true)}
            >
              {$t("llm.conv.always")}
            </button>
          {/if}
          <button
            type="button"
            class="button"
            onclick={() => void answerTool(ask!.tool_call_id, false)}
          >
            {$t("llm.conv.deny")}
          </button>
        </div>
      </div>
    {/if}
  </div>

  <Composer
    disabled={!agent}
    running={turn !== null}
    onsend={onSend}
    onstop={() => void cancelTurn()}
  />
</section>

<style>
  .tool-ask-preview {
    margin: var(--space-2) 0 0;
    padding: var(--space-2);
    max-height: 220px;
    overflow: auto;
    border-radius: var(--radius-sm);
    background: var(--fill-quaternary, rgba(127, 127, 127, 0.12));
    font-family: var(--font-mono, ui-monospace, monospace);
    font-size: var(--text-xs, 11px);
    white-space: pre-wrap;
    word-break: break-word;
  }
  .conv {
    flex: 1;
    min-width: 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }

  .conv-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-4);
    padding: var(--space-3) var(--space-5);
    border-bottom: var(--hairline) solid var(--separator);
  }

  .conv-title {
    display: flex;
    align-items: baseline;
    /* The workspace chip carries Undo: on a narrow window it drops to a line
       of its own instead of sliding under the actions on the right. */
    flex-wrap: wrap;
    row-gap: var(--space-1);
    gap: var(--space-3);
    min-width: 0;
    flex: 1 1 0;
  }

  .conv-title h2 {
    margin: 0;
    font-family: var(--font-display);
    font-size: var(--text-md);
    font-weight: 600;
    color: var(--text);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    /* The name is the last thing to give way: the model label shrinks first. */
    flex: 0 0 auto;
    max-width: 45%;
  }

  .conv-model {
    font-family: var(--font-mono);
    font-size: var(--text-caption);
    color: var(--text-dim);
    min-width: 0;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .conv-actions {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    flex-shrink: 0;
  }

  .conv-picker {
    padding: var(--space-3) var(--space-5);
    border-bottom: var(--hairline) solid var(--separator);
    background: var(--fill-1);
  }

  .conv-scroll {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: var(--space-3) var(--space-5);
    overscroll-behavior: contain;
  }

  .empty-state {
    padding: var(--space-7) 0;
    text-align: center;
  }

  .tool-ask {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-3);
    margin: var(--space-3) 0;
    padding: var(--space-3);
    border: var(--hairline) solid var(--separator);
    border-radius: var(--radius-md);
    background: var(--accent-soft);
  }

  .tool-ask-text {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }

  .tool-ask-title {
    font-size: var(--text-sm);
    font-weight: 600;
    color: var(--text);
  }

  .tool-ask-tool {
    font-family: var(--font-mono);
    font-size: var(--text-caption);
    color: var(--text-muted);
    overflow-wrap: anywhere;
  }

  .tool-ask-actions {
    display: flex;
    gap: var(--space-2);
    flex-shrink: 0;
  }

  .empty-state-body {
    margin: var(--space-2) 0 0;
    font-size: var(--text-sm);
    color: var(--text-dim);
  }
</style>
