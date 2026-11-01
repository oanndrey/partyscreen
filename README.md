# Tela P2P

Compartilhamento de tela no navegador com Vue 3 + WebRTC. O vídeo vai direto de quem compartilha para quem assiste; o servidor Node só faz a sinalização (troca de SDP/ICE via WebSocket).

## Rodar

```bash
npm install
npm run dev        # Vite em :5173 + sinalização em :3001
```

Produção:

```bash
npm run build
npm start          # serve dist/ + WebSocket na porta $PORT (padrão 3001)
```

## TURN (redes restritas)

Só STUN não atravessa NAT simétrico/firewalls corporativos. Configure um TURN (ex.: coturn):

```bash
TURN_URL=turn:turn.seudominio.com:3478 TURN_USER=usuario TURN_PASS=senha npm start
```

## Observações

- `getDisplayMedia` exige HTTPS (ou `localhost`). Para testar em outro aparelho da rede, use um proxy HTTPS ou um túnel.
- Topologia em estrela: o host abre uma conexão por espectador, então o upload dele limita quantas pessoas assistem bem (algo em torno de 5–10).
- Celulares normalmente só conseguem assistir; compartilhar tela pelo navegador é suportado só no desktop.
