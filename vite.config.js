import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'
import basicSsl from '@vitejs/plugin-basic-ssl'

// HTTPS é obrigatório para getDisplayMedia fora de localhost (acesso pela rede)
export default defineConfig({
  plugins: [vue(), basicSsl()],
  server: {
    host: true,
    port: 5180,
    strictPort: true,
    // Permite abrir pelo túnel (cloudflared) para testar com gente de fora da rede
    allowedHosts: ['.trycloudflare.com'],
    proxy: {
      '/api': 'http://localhost:3001',
      '/ws': { target: 'ws://localhost:3001', ws: true },
    },
  },
})
