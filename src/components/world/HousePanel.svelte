<script lang="ts">
  // Open house and visits. Nothing here talks to the network until a button is
  // pressed: opening asks the room server for a code, joining sends one.
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { t } from "$lib/i18n";
  import { showToast } from "$lib/stores/toast-store.svelte";

  type Visitor = { visitor_id: number; name: string };
  type HouseState = { role: "none" | "host" | "visitor"; code?: string; server?: string; ent?: number | null; host_name?: string | null; visitors: Visitor[] };
  type ChatLine = { from: number; name: string; text: string; ts: number };

  interface Props {
    /** Ask the page to show that house instead of home. */
    onvisit: (visit: { code: string; server: string | null }) => void;
    /** Ask the page to go back home. */
    onleave: () => void;
    /** A chat line arrived: the page puts it over the speaker's head. */
    onsay?: (ent: number, text: string) => void;
    visiting: boolean;
  }
  let { onvisit, onleave, onsay, visiting }: Props = $props();

  let house = $state<HouseState>({ role: "none", visitors: [] });
  let busy = $state(false);
  let code = $state("");
  let server = $state("");
  let showServer = $state(false);
  let chat = $state<ChatLine[]>([]);
  let draft = $state("");

  onMount(() => {
    const stops: Array<() => void> = [];
    void (async () => {
      try {
        house = (await invoke("house_status")) as HouseState;
      } catch {
        // Backend without the house commands: the panel stays in "none".
      }
      stops.push(await listen<HouseState>("house://state", (e) => (house = e.payload)));
      stops.push(
        await listen<ChatLine>("house://chat", (e) => {
          chat = [...chat.slice(-49), e.payload];
          if (e.payload.from > 0) onsay?.(1000 + e.payload.from, e.payload.text);
        }),
      );
      stops.push(
        await listen<{ reason: string; role: string }>("house://closed", (e) => {
          chat = [];
          if (e.payload.role === "visitor") {
            if (e.payload.reason !== "left") showToast("info", $t(`world.house.closed_${e.payload.reason}`) as string);
            onleave();
          }
        }),
      );
    })();
    return () => stops.forEach((stop) => stop());
  });

  async function openHouse() {
    busy = true;
    try {
      house = (await invoke("house_open", { server: server.trim() || null })) as HouseState;
    } catch (error) {
      showToast("error", String(error));
    } finally {
      busy = false;
    }
  }

  async function closeHouse() {
    house = (await invoke("house_close")) as HouseState;
  }

  function join() {
    const c = code.trim();
    if (!c) return;
    onvisit({ code: c, server: server.trim() || null });
    code = "";
  }

  async function copyCode() {
    try {
      await navigator.clipboard.writeText(house.code ?? "");
      showToast("success", $t("world.house.copied") as string);
    } catch {
      // Clipboard refused: the code is on screen anyway.
    }
  }

  async function sendChat() {
    const text = draft.trim();
    if (!text) return;
    draft = "";
    try {
      await invoke("house_chat", { text });
    } catch (error) {
      showToast("error", String(error));
    }
  }
</script>

<section class="house">
  {#if house.role === "host"}
    <div class="row">
      <span class="dot on" aria-hidden="true"></span>
      <span class="label">{$t("world.house.open_as")}</span>
      <button type="button" class="code" onclick={copyCode} title={$t("world.house.copy") as string}>{house.code}</button>
      <button type="button" class="btn" onclick={closeHouse}>{$t("world.house.close")}</button>
    </div>
  {:else if house.role === "visitor" || visiting}
    <div class="row">
      <span class="dot on" aria-hidden="true"></span>
      <span class="label">{$t("world.house.visiting", { name: house.host_name ?? "…" })}</span>
      <button type="button" class="btn" onclick={onleave}>{$t("world.house.leave")}</button>
    </div>
  {:else}
    <div class="row">
      <button type="button" class="btn primary" onclick={openHouse} disabled={busy}>
        {busy ? $t("world.house.opening") : $t("world.actions.open_house")}
      </button>
      <span class="sep">{$t("world.house.or")}</span>
      <form class="join" onsubmit={(e) => { e.preventDefault(); join(); }}>
        <input type="text" bind:value={code} placeholder="ABCD-EFGH" maxlength="12" spellcheck="false" autocapitalize="characters" aria-label={$t("world.house.code") as string} />
        <button type="submit" class="btn" disabled={!code.trim()}>{$t("world.house.join")}</button>
      </form>
      <button type="button" class="link" onclick={() => (showServer = !showServer)}>{$t("world.house.server")}</button>
    </div>
    {#if showServer}
      <div class="row">
        <input class="server" type="text" bind:value={server} placeholder="wss://chat.tonho.wtf" spellcheck="false" autocapitalize="off" />
        <span class="hint">{$t("world.house.server_hint")}</span>
      </div>
    {/if}
  {/if}

  {#if house.role !== "none"}
    <div class="rail" aria-label={$t("world.house.people") as string}>
      {#if house.role === "visitor"}<span class="chip host">{house.host_name}</span>{/if}
      {#each house.visitors as v (v.visitor_id)}
        <span class="chip"><span class="dot on" aria-hidden="true"></span>{v.name}</span>
      {:else}
        {#if house.role === "host"}<span class="hint">{$t("world.house.nobody")}</span>{/if}
      {/each}
    </div>
    <div class="chat">
      <div class="chat-log" aria-live="polite">
        {#each chat as line (line.ts + line.name + line.text)}
          <p><strong>{line.name}</strong> {line.text}</p>
        {/each}
      </div>
      <form class="chat-form" onsubmit={(e) => { e.preventDefault(); void sendChat(); }}>
        <input type="text" bind:value={draft} maxlength="500" placeholder={$t("world.house.chat_placeholder") as string} />
        <button type="submit" class="btn" disabled={!draft.trim()}>{$t("world.house.send")}</button>
      </form>
    </div>
  {/if}
</section>

<style>
  .house {
    display: flex;
    flex-direction: column;
    gap: 0.6rem;
    max-width: 960px;
  }
  .row,
  .join,
  .chat-form,
  .rail {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    flex-wrap: wrap;
  }
  .btn {
    padding: 0.35rem 0.8rem;
    border-radius: 8px;
    border: 1px solid rgba(127, 127, 127, 0.3);
    background: rgba(127, 127, 127, 0.12);
    color: inherit;
    font: inherit;
    font-size: 0.85rem;
    cursor: pointer;
  }
  .btn.primary {
    background: var(--accent, #0a84ff);
    border-color: transparent;
    color: #fff;
  }
  .btn:disabled {
    opacity: 0.5;
    cursor: default;
  }
  input {
    padding: 0.35rem 0.6rem;
    border-radius: 8px;
    border: 1px solid rgba(127, 127, 127, 0.3);
    background: rgba(127, 127, 127, 0.08);
    color: inherit;
    font: inherit;
    font-size: 0.85rem;
  }
  .join input {
    width: 8.5rem;
    font-family: ui-monospace, monospace;
    text-transform: uppercase;
  }
  .server {
    width: 18rem;
    font-family: ui-monospace, monospace;
  }
  .chat-form input {
    flex: 1;
    min-width: 0;
  }
  .code {
    font-family: ui-monospace, monospace;
    font-size: 1.05rem;
    letter-spacing: 0.08em;
    padding: 0.2rem 0.6rem;
    border-radius: 8px;
    border: 1px dashed rgba(127, 127, 127, 0.5);
    background: none;
    color: inherit;
    cursor: copy;
  }
  .link {
    background: none;
    border: 0;
    color: inherit;
    opacity: 0.6;
    font-size: 0.8rem;
    cursor: pointer;
    text-decoration: underline;
  }
  .sep,
  .hint,
  .label {
    font-size: 0.85rem;
    opacity: 0.7;
  }
  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: rgba(127, 127, 127, 0.5);
    display: inline-block;
  }
  .dot.on {
    background: #30d158;
  }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 0.35rem;
    padding: 0.15rem 0.6rem;
    border-radius: 999px;
    background: rgba(127, 127, 127, 0.14);
    font-size: 0.8rem;
  }
  .chip.host {
    font-weight: 600;
  }
  .chat-log {
    max-height: 9rem;
    overflow-y: auto;
    font-size: 0.85rem;
  }
  .chat-log p {
    margin: 0.15rem 0;
  }
</style>
