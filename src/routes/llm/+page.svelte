<script lang="ts">
  /**
   * Chat: rail, conversation, inspector. The roster loads once when the page
   * mounts; nothing here polls, and the only timer that can exist belongs to a
   * running turn.
   */
  import { onMount } from "svelte";
  import { t } from "$lib/i18n";
  import {
    getActiveTurn,
    getAgent,
    getAgents,
    getActiveAgentId,
    loadRoster,
    seedDemoConversation,
    newConversation,
    selectAgent,
  } from "$lib/stores/llm-store.svelte";
  import { loadProfile } from "$lib/stores/profile-store.svelte";
  import Rail from "$components/llm/Rail.svelte";
  import Conversation from "$components/llm/Conversation.svelte";
  import Inspector from "$components/llm/Inspector.svelte";

  let agents = $derived(getAgents());
  let activeAgent = $derived(getAgent(getActiveAgentId()));
  let turn = $derived(getActiveTurn());

  onMount(() => {
    void loadRoster().then(seedDemoConversation);
    void loadProfile();
  });

  function onNew() {
    const id = getActiveAgentId() ?? agents[0]?.id;
    if (id) newConversation(id);
  }
</script>

<svelte:head><title>{$t("llm.title")}</title></svelte:head>

<div class="llm-chat">
  <Rail {agents} speakingAgentId={turn?.agentId ?? null} onselect={selectAgent} onnew={onNew} />
  <Conversation agent={activeAgent} />
  <Inspector agent={activeAgent} />
</div>

<style>
  .llm-chat {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: row;
  }

  @media (max-width: 1100px) {
    .llm-chat :global(aside:last-child) {
      display: none;
    }
  }

  @media (max-width: 820px) {
    .llm-chat :global(aside:first-child) {
      display: none;
    }
  }
</style>
