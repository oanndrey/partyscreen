<script setup>
import { useRoute } from 'vue-router'

const route = useRoute()
</script>

<template>
  <div class="shell">
    <!-- Barra lateral como a do app -->
    <nav class="rail">
      <router-link to="/" class="rail-btn" :class="{ active: route.path === '/' }" title="Início">
        <img src="/logo.png" alt="PartyScreen" />
      </router-link>
      <div class="rail-sep"></div>
    </nav>
    <main class="content">
      <router-view />
    </main>
  </div>
</template>

<style scoped>
.shell { display: flex; height: 100%; }
.rail {
  width: 72px;
  flex-shrink: 0;
  background: var(--rail);
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 8px;
  padding: 12px 0;
}
.rail-btn {
  width: 48px;
  height: 48px;
  display: grid;
  place-items: center;
  border-radius: 24px;
  background: var(--bg);
  transition: border-radius .2s, background .2s;
}
.rail-btn img { width: 28px; height: 28px; transition: filter .2s; }
.rail-btn:hover, .rail-btn.active { border-radius: 16px; background: var(--primary); }
.rail-btn:hover img, .rail-btn.active img { filter: brightness(0) invert(1); }
.rail-sep { width: 32px; height: 2px; border-radius: 1px; background: rgb(255 255 255 / 10%); }
.content { flex: 1; min-width: 0; display: flex; flex-direction: column; overflow-y: auto; }

@media (max-width: 640px) {
  .rail { display: none; }
}
</style>
