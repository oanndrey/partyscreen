<script setup>
import { computed } from 'vue'

// Bolinha com a inicial do nick, na mesma cor que o app usa para esse nome
const props = defineProps({ name: { type: String, default: '' }, size: { type: Number, default: 32 } })
const COLORS = ['#5865f2', '#3ba55c', '#faa61a', '#ed4245', '#eb459e', '#fd5a10']
const color = computed(() => {
  let h = 0
  for (const b of new TextEncoder().encode(props.name)) h = (Math.imul(h, 31) + b) >>> 0
  return COLORS[h % COLORS.length]
})
</script>

<template>
  <div class="avatar" :style="{ background: color, width: size + 'px', height: size + 'px', fontSize: size * 0.42 + 'px' }">
    {{ name.charAt(0).toUpperCase() }}
  </div>
</template>

<style scoped>
.avatar {
  display: grid;
  place-items: center;
  flex-shrink: 0;
  border-radius: 50%;
  color: #fff;
  font-weight: 600;
}
</style>
