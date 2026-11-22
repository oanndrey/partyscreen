#!/bin/sh
# Gera o instalador do Windows e o app do Mac com a versão atual e põe na Área de Trabalho.
# Uso: ./release.sh 1.0.7   (sem número, mantém a versão atual)
set -e
cd "$(dirname "$0")"
if [ -n "$1" ]; then
  sed -i '' "s/\"version\": \"[0-9.]*\"/\"version\": \"$1\"/" src-tauri/tauri.conf.json
  sed -i '' "s/^version = \"[0-9.]*\"/version = \"$1\"/" src-tauri/Cargo.toml Cargo.toml
fi
V=$(grep '"version"' src-tauri/tauri.conf.json | head -1 | sed 's/.*"\([0-9.]*\)".*/\1/')
cd src-tauri
# FFmpeg do Windows (codificação na placa de vídeo); não vai pro git por ser grande
if [ ! -f windows-libs/ffmpeg.exe ]; then
  T=$(mktemp -d)
  gh release download latest -R BtbN/FFmpeg-Builds -p 'ffmpeg-n8.1-latest-win64-lgpl-8.1.zip' -D "$T"
  unzip -q -j "$T"/*.zip '*/bin/ffmpeg.exe' -d windows-libs
  rm -rf "$T"
fi
cargo tauri build --target x86_64-pc-windows-gnu --bundles nsis
cargo tauri build --bundles app
rm -f ~/Desktop/PartyScreen_*_x64-setup.exe
cp "target/x86_64-pc-windows-gnu/release/bundle/nsis/PartyScreen_${V}_x64-setup.exe" ~/Desktop/
osascript -e 'quit app "PartyScreen"' 2>/dev/null || true
sleep 1
rm -rf /Applications/PartyScreen.app
cp -R target/release/bundle/macos/PartyScreen.app /Applications/
echo "PartyScreen $V: Windows em ~/Desktop/PartyScreen_${V}_x64-setup.exe, Mac atualizado em Aplicativos"
