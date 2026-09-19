<script lang="ts">
  /**
   * Settings → Profile. Mounted by the settings route right after
   * SettingsAppearance, with no props. Loads the profile once when the panel is first shown;
   * no interval, no polling, nothing runs while the panel is closed.
   */
  import { t } from "$lib/i18n";
  import ProfileCard from "$components/profile/ProfileCard.svelte";
  import ProfileEditor from "$components/profile/ProfileEditor.svelte";
  import {
    getProfile,
    getProfileError,
    isProfileAvailable,
    isProfileLoading,
    loadProfile,
  } from "$lib/stores/profile-store.svelte";

  let profile = $derived(getProfile());
  let loading = $derived(isProfileLoading());
  let available = $derived(isProfileAvailable());
  let errorKey = $derived(getProfileError());

  $effect(() => {
    loadProfile();
  });
</script>

<section class="section">
  <div class="settings-section-head section-title">
    <h5 class="section-title">{$t("profile.title")}</h5>
    <p class="settings-section-hint">{$t("profile.desc")}</p>
  </div>

  {#if profile}
    <div class="card">
      <div class="setting-row">
        <ProfileCard {profile} />
      </div>
    </div>
    <div class="profile-gap"></div>
    <ProfileEditor />
  {:else if loading}
    <div class="card">
      <div class="setting-row">
        <span class="spinner"></span>
        <span class="setting-path">{$t("profile.loading")}</span>
      </div>
    </div>
  {:else}
    <div class="card">
      <div class="profile-empty">
        <span class="profile-empty-emoji" aria-hidden="true">🪪</span>
        <span class="setting-label">{$t("profile.empty_title")}</span>
        <span class="setting-path profile-empty-desc">
          {available && errorKey ? $t(errorKey) : $t("profile.empty_desc")}
        </span>
        <button class="button" onclick={() => loadProfile(true)}>
          {$t("profile.retry")}
        </button>
      </div>
    </div>
  {/if}
</section>

<style>
  .profile-gap {
    height: var(--space-3);
  }

  .profile-empty {
    display: flex;
    flex-direction: column;
    align-items: center;
    text-align: center;
    gap: var(--space-2);
    padding: var(--space-5) var(--space-3);
  }

  .profile-empty-emoji {
    font-size: 40px;
    line-height: 1;
  }

  .profile-empty-desc {
    white-space: normal;
    max-width: 320px;
  }
</style>
