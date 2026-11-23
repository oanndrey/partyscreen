<script setup>
// Fundo em degradê laranja + preto (manchas desfocadas, vidro canelado e base escurecida).
// `escurecer` (0 a 1) apaga o degradê nas telas da sala, para não competir com o vídeo.
defineProps({ escurecer: { type: Number, default: 0 } })

// [cx, cy, rx, ry, cor, opacidade, desfoque] num quadro de referência de 1440×900
const manchas = [
  [150, 80, 520, 400, '#000000', 0.95, 90],
  [1100, 200, 520, 420, '#fd5a10', 1, 90],
  [1350, 540, 360, 320, '#ff8a3d', 0.85, 100],
  [780, 430, 280, 240, '#ffb46b', 0.5, 85],
  [1180, -40, 260, 200, '#ffe0c2', 0.45, 80],
  [300, 760, 520, 300, '#c2410c', 0.35, 95],
  [700, 980, 700, 260, '#000000', 0.9, 90],
].map(([cx, cy, rx, ry, cor, opacidade, desfoque]) => ({
  left: `${((cx - rx) / 1440) * 100}%`,
  top: `${((cy - ry) / 900) * 100}%`,
  width: `${((rx * 2) / 1440) * 100}%`,
  height: `${((ry * 2) / 900) * 100}%`,
  background: cor,
  opacity: opacidade,
  filter: `blur(${desfoque}px)`,
}))
</script>

<template>
  <div class="fundo" aria-hidden="true">
    <span v-for="(m, i) in manchas" :key="i" class="mancha" :style="m"></span>
    <div class="canelado"></div>
    <div class="base"></div>
    <div v-if="escurecer" class="escuro" :style="{ opacity: escurecer }"></div>
  </div>
</template>

<style scoped>
.fundo { position: absolute; inset: 0; z-index: -1; overflow: hidden; background: #050505; pointer-events: none; }
.mancha { position: absolute; border-radius: 50%; }
.canelado {
  position: absolute;
  inset: 0;
  background-image: repeating-linear-gradient(90deg, rgb(255 255 255 / 0) 0px, rgb(255 255 255 / 0.07) 4.4px, rgb(0 0 0 / 0.08) 8px);
}
.base { position: absolute; inset: 42% 0 0; background: linear-gradient(rgb(5 5 5 / 0), rgb(5 5 5 / 0.85)); }
.escuro { position: absolute; inset: 0; background: #050506; }
</style>
