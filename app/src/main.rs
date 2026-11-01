// PartyScreen: interface (Leptos + componentes Rust/UI) rodando na janela Tauri.
// Compartilhar chama o backend Rust (captura + encoder + WebRTC);
// assistir usa o WebRTC da própria janela (public/viewer.js).
mod components;

use components::ui::button::{Button, ButtonSize, ButtonVariant};
use components::ui::slider::Slider;
use components::ui::spinner::Spinner;
use components::ui::switch::Switch;
use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

// ── Ponte com o Tauri e com o viewer.js ──────────────────────────────────

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = ["__TAURI__", "core"], catch)]
    async fn invoke(cmd: &str, args: JsValue) -> Result<JsValue, JsValue>;

    #[wasm_bindgen(js_namespace = ["__TAURI__", "event"])]
    async fn listen(event: &str, handler: &Closure<dyn FnMut(JsValue)>) -> JsValue;

    #[wasm_bindgen(js_namespace = telaViewer, js_name = start)]
    fn viewer_start(site: &str, code: &str, name: &str, video: &web_sys::HtmlVideoElement, on_status: &Closure<dyn FnMut(JsValue)>) -> JsValue;
}

fn to_js<T: Serialize>(value: &T) -> JsValue {
    value.serialize(&serde_wasm_bindgen::Serializer::json_compatible()).unwrap_or(JsValue::NULL)
}

fn error_text(e: JsValue) -> String {
    e.as_string().unwrap_or_else(|| "erro inesperado".into())
}

// ── Tipos trocados com o backend ─────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
enum Source {
    Display(usize),
    Window(u64),
}

/// App aberto que pode ser silenciado na transmissão
#[derive(Clone, Debug, Deserialize)]
struct AudioApp {
    bundle_id: String,
    name: String,
    suggested: bool,
}

#[derive(Clone, Debug, Deserialize)]
struct SourceInfo {
    source: Source,
    label: String,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ShareState {
    #[default]
    Idle,
    Connecting,
    Live {
        code: String,
        link: String,
    },
    Error {
        message: String,
    },
}

#[derive(Clone, Debug, Default, Deserialize)]
struct ShareStatus {
    state: ShareState,
    fps: f64,
    mbps: f64,
    encode_ms: f64,
    #[serde(default)]
    routes: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    encoder: String,
    #[serde(default)]
    members: Vec<Member>,
    #[serde(default)]
    chat: Vec<ChatMsg>,
}

/// Mensagem do chat da sala (system = aviso de entrada/saída)
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
struct ChatMsg {
    id: String,
    name: String,
    text: String,
    ts: f64,
    system: bool,
}

/// Alguém na sala (vem do servidor): role "host" transmite, "viewer" assiste
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
struct Member {
    id: String,
    name: String,
    role: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct ViewerStatus {
    state: Option<String>,
    message: Option<String>,
    fps: Option<f64>,
    mbps: Option<f64>,
    route: Option<String>,
    members: Option<Vec<Member>>,
    /// Meu id na sala
    me: Option<String>,
    chat: Option<ChatMsg>,
}

/// Bolinha com a inicial do nick, numa cor fixa por nome (como avatar padrão do Discord)
fn avatar(name: &str, size: &'static str) -> impl IntoView + use<> {
    const COLORS: [&str; 6] = ["bg-[#5865f2]", "bg-[#3ba55c]", "bg-[#faa61a]", "bg-[#ed4245]", "bg-[#eb459e]", "bg-[#fd5a10]"];
    let hash = name.bytes().fold(0usize, |h, b| h.wrapping_mul(31).wrapping_add(b as usize));
    let initial = name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
    view! {
        <div class=format!("flex justify-center items-center font-semibold text-white rounded-full shrink-0 {size} {}", COLORS[hash % COLORS.len()])>
            {initial}
        </div>
    }
}

/// Texto e cor (classe da bolinha) do caminho da conexão
fn route_label(route: &str) -> (&'static str, &'static str) {
    match route {
        "local" => ("Direta · mesma rede", "bg-success"),
        "direct" => ("Direta · internet", "bg-success"),
        "direct-ipv6" => ("Direta · IPv6", "bg-success"),
        "relay" => ("Retransmitida (TURN)", "bg-warning"),
        _ => ("Conectando…", "bg-muted-foreground"),
    }
}

// ── Configurações (salvas no navegador da janela) ────────────────────────

/// Servidor de sinalização (Cloudflare Worker em worker/)
const SERVER: &str = "https://partyscreen.partyscreen.workers.dev";

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    site: String,
    fps: u32,
    mbps: u32,
    audio: bool,
    /// Largura máxima enviada: 1280 (720p) ou 1920 (1080p)
    max_width: u32,
    /// Prioriza nitidez (pode pular quadros) em vez de fluidez
    sharp: bool,
    /// Como os amigos te veem na sala
    nick: String,
    /// Apps cujo som não vai para a transmissão (bundle IDs no Mac)
    muted_apps: Vec<String>,
    /// false = ainda não mexeu: os apps de chamada (Discord etc.) já vêm silenciados
    muted_set: bool,
}

impl Default for Settings {
    fn default() -> Self {
        // 720p a 60 fps cabe folgado no encoder por software; 1080p pode engasgar em jogo
        Self { site: SERVER.into(), fps: 60, mbps: 6, audio: true, max_width: 1280, sharp: false, nick: String::new(), muted_apps: Vec::new(), muted_set: false }
    }
}

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok()?
}

fn load_settings() -> Settings {
    storage()
        .and_then(|s| s.get_item("settings").ok()?)
        .and_then(|json| serde_json_wasm(&json))
        .map(|s| Settings { site: SERVER.into(), ..s })
        .unwrap_or_default()
}

fn serde_json_wasm(json: &str) -> Option<Settings> {
    let value = js_sys::JSON::parse(json).ok()?;
    serde_wasm_bindgen::from_value(value).ok()
}

fn save_settings(s: &Settings) {
    if let (Some(storage), Ok(json)) = (storage(), js_sys::JSON::stringify(&to_js(s))) {
        let _ = storage.set_item("settings", &String::from(json));
    }
}

fn copy_to_clipboard(text: &str) {
    if let Some(w) = web_sys::window() {
        let _ = w.navigator().clipboard().write_text(text);
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Page {
    Home,
    Picking,
    Sharing,
    Watching,
}

#[derive(Clone, Copy)]
struct Ctx {
    page: RwSignal<Page>,
    settings: RwSignal<Settings>,
    show_settings: RwSignal<bool>,
    code: RwSignal<String>,
    toast: RwSignal<Option<String>>,
    /// Estado da transmissão (vem do backend por evento; vale em qualquer tela)
    share: RwSignal<ShareStatus>,
    sharing: RwSignal<bool>,
    /// Pedindo o nick antes de ir para esta tela (None = janela fechada)
    ask_nick: RwSignal<Option<Page>>,
    /// O que está sendo transmitido (para a prévia na sala)
    live_source: RwSignal<Option<SourceInfo>>,
}

/// Vai para a tela, pedindo o nick antes quando precisa
fn go(ctx: Ctx, page: Page) {
    // Ao entrar numa sala sempre confirma o nick (já vem preenchido); para transmitir, só se não tiver
    let needs_nick = page == Page::Watching
        || (page == Page::Picking && ctx.settings.with_untracked(|s| s.nick.trim().is_empty()));
    if needs_nick {
        ctx.ask_nick.set(Some(page));
    } else {
        ctx.page.set(page);
    }
}

fn show_toast(ctx: Ctx, text: &str) {
    ctx.toast.set(Some(text.to_string()));
    set_timeout(move || ctx.toast.set(None), std::time::Duration::from_millis(1800));
}

fn main() {
    console_error_panic_hook_set();
    mount_to_body(App);
}

fn console_error_panic_hook_set() {
    std::panic::set_hook(Box::new(|info| {
        web_sys::console::error_1(&info.to_string().into());
    }));
}

fn stop_sharing(ctx: Ctx) {
    leptos::task::spawn_local(async move {
        let _ = invoke("stop_share", JsValue::NULL).await;
    });
    ctx.sharing.set(false);
    ctx.share.set(ShareStatus::default());
    ctx.page.set(Page::Home);
}

#[component]
fn App() -> impl IntoView {
    let settings = RwSignal::new(load_settings());
    let ctx = Ctx {
        page: RwSignal::new(Page::Home),
        show_settings: RwSignal::new(false),
        settings,
        code: RwSignal::new(String::new()),
        toast: RwSignal::new(None),
        share: RwSignal::new(ShareStatus::default()),
        sharing: RwSignal::new(false),
        ask_nick: RwSignal::new(None),
        live_source: RwSignal::new(None),
    };
    provide_context(ctx);
    Effect::new(move |_| save_settings(&settings.get()));

    // Registrado uma vez: dá para navegar entre telas sem perder o estado da transmissão
    let handler = Closure::<dyn FnMut(JsValue)>::new(move |event: JsValue| {
        let Ok(payload) = js_sys::Reflect::get(&event, &"payload".into()) else { return };
        let Ok(s) = serde_wasm_bindgen::from_value::<ShareStatus>(payload) else { return };
        if let ShareState::Error { message } = &s.state {
            show_toast(ctx, message);
            ctx.sharing.set(false);
            if ctx.page.get_untracked() == Page::Sharing {
                ctx.page.set(Page::Home);
            }
        }
        ctx.share.set(s);
    });
    leptos::task::spawn_local(async move {
        let _ = listen("share-status", &handler).await;
        handler.forget();
    });

    view! {
        <div class="flex overflow-hidden h-screen bg-background text-foreground select-none">
            <Rail />
            <main class="flex flex-col flex-1 min-w-0">
                {move || match ctx.page.get() {
                    Page::Home => view! { <Home /> }.into_any(),
                    Page::Picking => view! { <Picking /> }.into_any(),
                    Page::Sharing => view! { <Sharing /> }.into_any(),
                    Page::Watching => view! { <Watching /> }.into_any(),
                }}
            </main>
            <SettingsModal />
            <NickModal />
            <Toast />
        </div>
    }
}

// ── Barra lateral (estilo Discord) ───────────────────────────────────────

#[component]
fn RailButton(
    #[prop(into)] title: String,
    #[prop(into)] active: Signal<bool>,
    #[prop(into, optional)] live: Signal<bool>,
    on_click: Callback<()>,
    children: Children,
) -> impl IntoView {
    view! {
        <div class="flex relative justify-center w-full group">
            <span class=move || format!(
                "absolute left-0 top-1/2 w-1 rounded-r-full -translate-y-1/2 bg-foreground transition-all duration-200 {}",
                if active.get() { "h-10" } else { "h-0 group-hover:h-5" }
            )></span>
            <button
                title=title
                class=move || format!(
                    "flex justify-center items-center size-12 transition-all duration-200 {}",
                    // Logo (imagem) fica branco quando o fundo vira laranja, senão some
                    if active.get() {
                        "rounded-2xl bg-primary text-primary-foreground [&_img]:brightness-0 [&_img]:invert"
                    } else {
                        "rounded-[24px] bg-background text-foreground/80 hover:rounded-2xl hover:bg-primary hover:text-primary-foreground hover:[&_img]:brightness-0 hover:[&_img]:invert"
                    }
                )
                on:click=move |_| on_click.run(())
            >
                {children()}
            </button>
            <Show when=move || live.get()>
                <span class="absolute bottom-0 right-3 rounded-full border-4 size-4 bg-destructive border-rail"></span>
            </Show>
        </div>
    }
}

#[component]
fn Rail() -> impl IntoView {
    let ctx = expect_context::<Ctx>();
    let page = move || ctx.page.get();
    view! {
        <nav class="flex flex-col gap-2 items-center py-3 w-[72px] shrink-0 bg-rail">
            <RailButton
                title="Início"
                active=Signal::derive(move || matches!(page(), Page::Home | Page::Watching))
                on_click=Callback::new(move |_| ctx.page.set(Page::Home))
            >
                <img src="/logo.png" alt="" class="size-7" />
            </RailButton>
            <div class="w-8 h-0.5 rounded-full bg-white/10"></div>
            <RailButton
                title="Transmitir"
                active=Signal::derive(move || matches!(page(), Page::Picking | Page::Sharing))
                live=Signal::derive(move || ctx.sharing.get())
                on_click=Callback::new(move |_| {
                    go(ctx, if ctx.sharing.get_untracked() { Page::Sharing } else { Page::Picking })
                })
            >
                <icons::ScreenShare class="size-5" />
            </RailButton>
            <div class="flex-1"></div>
            {move || {
                let nick = ctx.settings.with(|s| s.nick.clone());
                (!nick.is_empty()).then(move || view! {
                    <button title=format!("{nick} · trocar nick") class="relative rounded-full transition-opacity hover:opacity-80" on:click=move |_| ctx.ask_nick.set(Some(ctx.page.get_untracked()))>
                        {avatar(&nick, "size-10 text-base")}
                        <span class="absolute right-0 bottom-0 rounded-full border-[3px] size-3.5 bg-success border-rail"></span>
                    </button>
                })
            }}
            <RailButton
                title="Configurações"
                active=Signal::derive(move || ctx.show_settings.get())
                on_click=Callback::new(move |_| ctx.show_settings.set(true))
            >
                <icons::Settings class="size-5" />
            </RailButton>
        </nav>
    }
}

/// Barra do topo de cada tela (como o cabeçalho de canal do Discord)
#[component]
fn TopBar(#[prop(into)] title: String, children: Children) -> impl IntoView {
    view! {
        <header class="flex gap-2 items-center px-4 h-12 border-b shadow-sm shrink-0 border-black/25">
            {children()}
            <h1 class="text-[15px] font-semibold text-accent-foreground">{title}</h1>
        </header>
    }
}

fn section_label(text: &'static str) -> impl IntoView {
    view! { <div class="mb-2 text-xs font-bold tracking-wide uppercase text-muted-foreground">{text}</div> }
}

// ── Início ───────────────────────────────────────────────────────────────

#[component]
fn Home() -> impl IntoView {
    let ctx = expect_context::<Ctx>();
    let can_watch = move || ctx.code.with(|c| c.trim().len() >= 4);
    let watch = move || {
        if can_watch() {
            ctx.code.update(|c| *c = c.trim().to_uppercase());
            go(ctx, Page::Watching);
        }
    };

    view! {
        <TopBar title="Início"><icons::House class="size-5 text-muted-foreground" /></TopBar>
        <div class="overflow-y-auto flex-1">
            <div class="py-10 px-8 mx-auto max-w-3xl">
                <Show when=move || ctx.sharing.get()>
                    <button
                        class="flex gap-3 items-center py-2.5 px-4 mb-6 w-full text-sm text-left rounded-md transition-colors bg-destructive/15 hover:bg-destructive/25"
                        on:click=move |_| ctx.page.set(Page::Sharing)
                    >
                        <span class="rounded-full animate-pulse size-2 bg-destructive"></span>
                        <span class="flex-1 font-medium">"Você está transmitindo agora"</span>
                        <span class="text-muted-foreground">"Abrir"</span>
                        <icons::ChevronRight class="size-4 text-muted-foreground" />
                    </button>
                </Show>

                <h2 class="text-2xl font-bold text-accent-foreground">"Bem-vindo ao PartyScreen"</h2>
                <p class="mt-1 text-muted-foreground">"Transmita sua tela para os amigos com um código. Sem cadastro."</p>

                <div class="grid gap-4 mt-8 md:grid-cols-2">
                    <button
                        class="flex flex-col gap-4 items-start p-5 text-left rounded-lg transition-colors group bg-card hover:bg-accent"
                        on:click=move |_| go(ctx, if ctx.sharing.get_untracked() { Page::Sharing } else { Page::Picking })
                    >
                        <div class="flex justify-center items-center rounded-full size-12 bg-primary/15 text-primary">
                            <icons::ScreenShare class="size-6" />
                        </div>
                        <div>
                            <div class="font-semibold text-accent-foreground">"Transmitir minha tela"</div>
                            <div class="mt-0.5 text-sm text-muted-foreground">"Escolha uma tela ou janela e gere um código."</div>
                        </div>
                        <div class="flex gap-1 items-center mt-auto text-sm font-medium text-primary">
                            "Começar"<icons::ChevronRight class="transition-transform size-4 group-hover:translate-x-0.5" />
                        </div>
                    </button>

                    <div class="flex flex-col gap-4 p-5 rounded-lg bg-card">
                        <div class="flex justify-center items-center rounded-full size-12 bg-success/15 text-success">
                            <icons::MonitorPlay class="size-6" />
                        </div>
                        <div>
                            <div class="font-semibold text-accent-foreground">"Entrar numa sala"</div>
                            <div class="mt-0.5 text-sm text-muted-foreground">"Digite o código que você recebeu."</div>
                        </div>
                        <div class="flex gap-2 mt-auto">
                            <input
                                class="flex-1 px-3 min-w-0 h-10 font-mono text-base font-semibold tracking-[0.3em] uppercase rounded outline-none bg-input text-accent-foreground placeholder:tracking-normal placeholder:font-sans placeholder:font-normal placeholder:text-muted-foreground focus:ring-2 focus:ring-primary"
                                placeholder="Ex.: K7QX2M"
                                maxlength="6"
                                prop:value=move || ctx.code.get()
                                on:input=move |ev| ctx.code.set(event_target_value(&ev).to_uppercase())
                                on:keydown=move |ev| if ev.key() == "Enter" { watch() }
                            />
                            <Button class="h-10" attr:disabled=move || !can_watch() on:click=move |_| watch()>"Entrar"</Button>
                        </div>
                    </div>
                </div>

                <div class="mt-10">{section_label("Como funciona")}</div>
                <div class="grid gap-3 sm:grid-cols-3">
                    {[
                        ("1", "Transmita", "Escolha a tela ou a janela, com ou sem som."),
                        ("2", "Mande o código", "Seu amigo digita o código aqui ou abre o link."),
                        ("3", "Assistam juntos", "Vídeo direto entre vocês, em tempo real."),
                    ].into_iter().map(|(n, t, d)| view! {
                        <div class="flex gap-3 p-4 rounded-lg bg-card">
                            <span class="flex justify-center items-center text-xs font-bold rounded-full size-6 shrink-0 bg-elevated text-muted-foreground">{n}</span>
                            <div>
                                <div class="text-sm font-semibold text-accent-foreground">{t}</div>
                                <div class="mt-0.5 text-xs leading-relaxed text-muted-foreground">{d}</div>
                            </div>
                        </div>
                    }).collect_view()}
                </div>
            </div>
        </div>
    }
}

// ── Escolher o que transmitir (estilo "Go Live") ──────────────────────────

/// Grupo de botões lado a lado (só um ativo)
fn segmented<T: Copy + PartialEq + Send + Sync + 'static>(
    options: Vec<(T, &'static str)>,
    current: Signal<T>,
    set: impl Fn(T) + Copy + Send + Sync + 'static,
) -> impl IntoView {
    view! {
        <div class="flex gap-0.5 p-0.5 rounded-md bg-rail">
            {options.into_iter().map(|(value, label)| view! {
                <button
                    class=move || format!(
                        "px-3 h-8 text-sm font-medium rounded transition-colors {}",
                        if current.get() == value { "bg-accent text-accent-foreground" } else { "text-muted-foreground hover:text-accent-foreground" }
                    )
                    on:click=move |_| set(value)
                >
                    {label}
                </button>
            }).collect_view()}
        </div>
    }
}

#[component]
fn Picking() -> impl IntoView {
    let ctx = expect_context::<Ctx>();
    let sources = RwSignal::new(Vec::<SourceInfo>::new());
    let error = RwSignal::new(None::<String>);
    let loading = RwSignal::new(true);
    let selected = RwSignal::new(None::<Source>);
    let screens_tab = RwSignal::new(true);
    let audio = RwSignal::new(ctx.settings.get_untracked().audio);
    Effect::new(move |_| {
        let a = audio.get();
        ctx.settings.update(|s| s.audio = a);
    });

    let apps = RwSignal::new(Vec::<AudioApp>::new());
    let show_apps = RwSignal::new(false);

    let refresh = move || {
        loading.set(true);
        leptos::task::spawn_local(async move {
            let Ok(v) = invoke("list_audio_apps", JsValue::NULL).await else { return };
            let list: Vec<AudioApp> = serde_wasm_bindgen::from_value(v).unwrap_or_default();
            // Primeira vez: Discord, TeamSpeak & cia. já começam silenciados
            if !ctx.settings.with_untracked(|s| s.muted_set) {
                let suggested: Vec<String> = list.iter().filter(|a| a.suggested).map(|a| a.bundle_id.clone()).collect();
                ctx.settings.update(|s| {
                    for id in suggested {
                        if !s.muted_apps.contains(&id) {
                            s.muted_apps.push(id);
                        }
                    }
                });
            }
            apps.set(list);
        });
        leptos::task::spawn_local(async move {
            match invoke("list_sources", JsValue::NULL).await {
                Ok(v) => {
                    let list: Vec<SourceInfo> = serde_wasm_bindgen::from_value(v).unwrap_or_default();
                    if !selected.with_untracked(|s| s.as_ref().is_some_and(|s| list.iter().any(|i| &i.source == s))) {
                        selected.set(list.first().map(|i| i.source.clone()));
                    }
                    sources.set(list);
                    error.set(None);
                }
                Err(e) => error.set(Some(error_text(e))),
            }
            loading.set(false);
        });
    };
    refresh();

    let share = move || {
        let Some(source) = selected.get_untracked() else { return };
        let s = ctx.settings.get_untracked();
        let config = serde_json_value(&source, &s);
        ctx.live_source.set(sources.with_untracked(|l| l.iter().find(|i| i.source == source).cloned()));
        ctx.sharing.set(true);
        ctx.share.set(ShareStatus { state: ShareState::Connecting, ..Default::default() });
        ctx.page.set(Page::Sharing);
        leptos::task::spawn_local(async move {
            let _ = invoke("start_share", config).await;
        });
    };

    let count = move |screens: bool| sources.with(|l| l.iter().filter(|i| matches!(i.source, Source::Display(_)) == screens).count());
    let tab = move |screens: bool, label: &'static str| view! {
        <button
            class=move || format!(
                "relative py-3 text-sm font-medium transition-colors {}",
                if screens_tab.get() == screens { "text-accent-foreground" } else { "text-muted-foreground hover:text-accent-foreground" }
            )
            on:click=move |_| screens_tab.set(screens)
        >
            {label}
            <span class="ml-1.5 text-xs text-muted-foreground">{move || count(screens)}</span>
            <Show when=move || screens_tab.get() == screens>
                <span class="absolute inset-x-0 bottom-0 h-0.5 rounded-full bg-primary"></span>
            </Show>
        </button>
    };

    let settings = ctx.settings;
    view! {
        <TopBar title="Transmitir"><icons::ScreenShare class="size-5 text-muted-foreground" /></TopBar>

        <div class="flex gap-6 items-center px-6 border-b border-black/20 shrink-0">
            {tab(true, "Telas")}
            {tab(false, "Janelas")}
            <div class="flex-1"></div>
            <button class="flex gap-1.5 items-center text-sm transition-colors text-muted-foreground hover:text-accent-foreground" on:click=move |_| refresh()>
                <icons::RefreshCw class="size-4" />"Atualizar"
            </button>
        </div>

        <div class="overflow-y-auto flex-1 p-6">
            {move || error.get().map(|e| view! {
                <div class="py-3 px-4 mb-4 text-sm rounded-md bg-destructive/15">{e}</div>
            })}
            <Show when=move || loading.get() && sources.with(|s| s.is_empty())>
                <div class="flex justify-center py-16"><Spinner /></div>
            </Show>
            <div class="grid gap-x-4 gap-y-5 sm:grid-cols-2 lg:grid-cols-3">
                {move || {
                    let screens = screens_tab.get();
                    sources.get().into_iter()
                        .filter(|i| matches!(i.source, Source::Display(_)) == screens)
                        .map(|item| source_tile(item, selected, share))
                        .collect_view()
                }}
            </div>
        </div>

        <div class="py-4 px-6 border-t shrink-0 bg-card border-black/20">
            <div class="flex flex-wrap gap-x-6 gap-y-3 items-end">
                <div>
                    {section_label("Resolução")}
                    {segmented(vec![(1280u32, "720p"), (1920, "1080p")], Signal::derive(move || settings.with(|s| s.max_width)), move |v| settings.update(|s| s.max_width = v))}
                </div>
                <div>
                    {section_label("Quadros")}
                    {segmented(vec![(30u32, "30 fps"), (60, "60 fps")], Signal::derive(move || settings.with(|s| s.fps)), move |v| settings.update(|s| s.fps = v))}
                </div>
                <div>
                    {section_label("Prioridade")}
                    {segmented(vec![(false, "Fluidez"), (true, "Nitidez")], Signal::derive(move || settings.with(|s| s.sharp)), move |v| settings.update(|s| s.sharp = v))}
                </div>
                <div>
                    {section_label("Som")}
                    <div class="flex gap-2 items-center h-9">
                        <Switch checked=audio aria_label="Transmitir o som do computador" />
                        <span class="text-sm text-muted-foreground">{move || if audio.get() { "Ligado" } else { "Desligado" }}</span>
                    </div>
                </div>
                <Show when=move || audio.get() && apps.with(|a| !a.is_empty())>
                    <div class="relative">
                        {section_label("Sem som de")}
                        <button
                            class="flex gap-2 items-center px-3 h-9 text-sm rounded-md transition-colors bg-elevated hover:bg-accent max-w-[220px]"
                            on:click=move |_| show_apps.update(|v| *v = !*v)
                        >
                            <icons::VolumeX class="size-4 shrink-0 text-muted-foreground" />
                            <span class="truncate">{move || {
                                let names: Vec<String> = apps.with(|list| settings.with(|s| {
                                    list.iter().filter(|a| s.muted_apps.contains(&a.bundle_id)).map(|a| a.name.clone()).collect()
                                }));
                                match names.len() {
                                    0 => "Nenhum app".to_string(),
                                    1 => names[0].clone(),
                                    n => format!("{} +{}", names[0], n - 1),
                                }
                            }}</span>
                            <icons::ChevronDown class="size-4 shrink-0 text-muted-foreground" />
                        </button>
                        <Show when=move || show_apps.get()>
                            <div class="fixed inset-0 z-40" on:click=move |_| show_apps.set(false)></div>
                            <div class="overflow-y-auto absolute left-0 bottom-full z-50 p-2 mb-2 w-72 max-h-80 rounded-lg shadow-2xl bg-rail animate-in fade-in-0 slide-in-from-bottom-2">
                                <div class="px-2 pt-1 pb-2 text-xs text-muted-foreground">"O som desses apps não vai para a transmissão (a janela continua aparecendo)."</div>
                                {move || apps.get().into_iter().map(|app| {
                                    let id = app.bundle_id.clone();
                                    let id2 = id.clone();
                                    let muted = move || settings.with(|s| s.muted_apps.contains(&id));
                                    view! {
                                        <button
                                            class="flex gap-3 items-center py-2 px-2 w-full text-sm text-left rounded transition-colors hover:bg-accent"
                                            on:click=move |_| settings.update(|s| {
                                                s.muted_set = true;
                                                if let Some(i) = s.muted_apps.iter().position(|m| m == &id2) {
                                                    s.muted_apps.remove(i);
                                                } else {
                                                    s.muted_apps.push(id2.clone());
                                                }
                                            })
                                        >
                                            <span class=move || format!(
                                                "flex justify-center items-center rounded size-5 shrink-0 {}",
                                                if muted() { "bg-primary text-primary-foreground" } else { "border-2 border-muted-foreground/50" }
                                            )>
                                                <Show when=muted.clone()><icons::Check class="size-3.5" /></Show>
                                            </span>
                                            <span class="flex-1 truncate text-accent-foreground">{app.name.clone()}</span>
                                            {app.suggested.then(|| view! { <span class="text-[11px] text-muted-foreground">"chamada"</span> })}
                                        </button>
                                    }
                                }).collect_view()}
                            </div>
                        </Show>
                    </div>
                </Show>
                <div class="flex-1"></div>
                <Button size=ButtonSize::Lg class="px-8 font-semibold" attr:disabled=move || selected.with(|s| s.is_none()) on:click=move |_| share()>
                    <icons::Radio />"Transmitir"
                </Button>
            </div>
        </div>
    }
}

fn serde_json_value(source: &Source, s: &Settings) -> JsValue {
    #[derive(Serialize)]
    struct Config<'a> {
        source: &'a Source,
        audio: bool,
        site: &'a str,
        fps: u32,
        mbps: u32,
        max_width: u32,
        sharp: bool,
        name: &'a str,
        muted_apps: &'a [String],
    }
    #[derive(Serialize)]
    struct Args<'a> {
        config: Config<'a>,
    }
    to_js(&Args {
        config: Config { source, audio: s.audio, site: &s.site, fps: s.fps, mbps: s.mbps, max_width: s.max_width, sharp: s.sharp, name: s.nick.trim(), muted_apps: &s.muted_apps },
    })
}

fn source_tile(item: SourceInfo, selected: RwSignal<Option<Source>>, share: impl Fn() + Copy + 'static) -> impl IntoView {
    let is_screen = matches!(item.source, Source::Display(_));
    let (title, detail) = match item.label.split_once(" — ") {
        Some((app, title)) => (title.to_string(), app.to_string()),
        None => (item.label.clone(), if is_screen { "Tela inteira".into() } else { "Janela".into() }),
    };
    let source = item.source.clone();
    let source2 = item.source.clone();
    // Memo é Copy: pode ser usado em várias classes reativas
    let active = Memo::new(move |_| selected.with(|s| s.as_ref() == Some(&source)));
    let active = move || active.get();

    // Cada bloco busca a própria miniatura, sem travar a lista
    let thumb = RwSignal::new(None::<String>);
    let thumb_source = item.source.clone();
    leptos::task::spawn_local(async move {
        #[derive(Serialize)]
        struct Args<'a> {
            source: &'a Source,
        }
        if let Ok(v) = invoke("thumbnail", to_js(&Args { source: &thumb_source })).await {
            if let Some(url) = v.as_string() {
                thumb.try_set(Some(url));
            }
        }
    });
    view! {
        <button
            type="button"
            title=item.label.clone()
            class="flex flex-col gap-2 min-w-0 text-left group"
            on:click=move |_| selected.set(Some(source2.clone()))
            on:dblclick=move |_| share()
        >
            <div class=move || format!(
                "flex overflow-hidden relative justify-center items-center w-full rounded-lg ring-2 transition-all aspect-video bg-rail {}",
                if active() { "ring-primary" } else { "ring-transparent group-hover:ring-white/15" }
            )>
                {move || match thumb.get() {
                    Some(url) => view! {
                        <img src=url alt="" draggable="false" class="object-contain w-full h-full animate-in fade-in-0" />
                    }.into_any(),
                    None => view! {
                        <div class=move || format!("transition-colors {}", if active() { "text-primary" } else { "text-muted-foreground/60" })>
                            {if is_screen { view! { <icons::Monitor class="size-10" /> }.into_any() } else { view! { <icons::AppWindow class="size-10" /> }.into_any() }}
                        </div>
                    }.into_any(),
                }}
                // Ícone do tipo no canto, por cima da miniatura
                <span class="flex absolute bottom-1.5 left-1.5 justify-center items-center rounded size-6 bg-black/60 text-white/90">
                    {if is_screen { view! { <icons::Monitor class="size-3.5" /> }.into_any() } else { view! { <icons::AppWindow class="size-3.5" /> }.into_any() }}
                </span>
            </div>
            <div class="px-0.5 min-w-0">
                <div class=move || format!("text-sm font-medium truncate {}", if active() { "text-accent-foreground" } else { "text-foreground" })>{title}</div>
                <div class="text-xs truncate text-muted-foreground">{detail}</div>
            </div>
        </button>
    }
}

// ── Sala (transmitindo ou assistindo, no estilo de uma call do Discord) ──

fn live_pill() -> impl IntoView {
    view! { <span class="py-0.5 px-1.5 text-[11px] font-bold tracking-wide text-white rounded bg-destructive">"AO VIVO"</span> }
}

/// Hora da mensagem (hh:mm) a partir do timestamp em ms
fn chat_time(ts: f64) -> String {
    let d = js_sys::Date::new(&JsValue::from_f64(ts));
    format!("{:02}:{:02}", d.get_hours(), d.get_minutes())
}

/// Cabeçalho da sala: código e convite sempre à mão
#[component]
fn RoomHeader(#[prop(into)] code: Signal<String>, #[prop(into)] link: Signal<String>) -> impl IntoView {
    let ctx = expect_context::<Ctx>();
    view! {
        <header class="flex gap-2 items-center px-4 h-12 border-b shadow-sm shrink-0 border-black/25">
            <icons::Volume2 class="size-5 text-muted-foreground" />
            <h1 class="text-[15px] font-semibold text-accent-foreground">"Sala"</h1>
            <button
                title="Copiar código"
                class="flex gap-1.5 items-center py-1 px-2 font-mono text-sm font-semibold tracking-widest rounded transition-colors bg-rail text-accent-foreground hover:bg-accent"
                on:click=move |_| { copy_to_clipboard(&code.get_untracked()); show_toast(ctx, "Código copiado") }
            >
                {move || code.get()}<icons::Copy class="size-3.5 text-muted-foreground" />
            </button>
            <div class="flex-1"></div>
            <Button size=ButtonSize::Sm on:click=move |_| { copy_to_clipboard(&link.get_untracked()); show_toast(ctx, "Convite copiado") }>
                <icons::UserPlus />"Convidar"
            </Button>
        </header>
    }
}

/// Coluna da direita: quem está na sala + chat
#[component]
fn RoomPanel(
    #[prop(into)] members: Signal<Vec<Member>>,
    #[prop(into)] me: Signal<String>,
    #[prop(into)] routes: Signal<std::collections::BTreeMap<String, String>>,
    #[prop(into)] chat: Signal<Vec<ChatMsg>>,
    on_send: Callback<String>,
) -> impl IntoView {
    let draft = RwSignal::new(String::new());
    let list = NodeRef::<leptos::html::Div>::new();
    // Rola para a última mensagem quando chega uma nova
    Effect::new(move |_| {
        chat.with(|c| c.len());
        set_timeout(move || {
            if let Some(el) = list.get_untracked() {
                el.set_scroll_top(el.scroll_height());
            }
        }, std::time::Duration::from_millis(0));
    });
    let send = move || {
        let text = draft.get_untracked().trim().to_string();
        if !text.is_empty() {
            on_send.run(text);
            draft.set(String::new());
        }
    };

    view! {
        <aside class="flex flex-col w-80 border-l shrink-0 bg-card border-black/25">
            <div class="overflow-y-auto p-3 border-b max-h-[40%] shrink-0 border-black/25">
                <div class="px-2 pb-1 text-xs font-bold tracking-wide uppercase text-muted-foreground">
                    {move || format!("Na sala — {}", members.with(|m| m.len()))}
                </div>
                {move || {
                    let me = me.get();
                    let routes = routes.get();
                    members.get().into_iter().map(|m| {
                        let host = m.role == "host";
                        let you = m.id == me;
                        let sub = if host {
                            Some(("Transmitindo", "text-destructive"))
                        } else {
                            routes.get(&m.id).map(|r| (route_label(r).0, "text-muted-foreground"))
                        };
                        view! {
                            <div class="flex gap-3 items-center py-1 px-2 rounded hover:bg-accent/50">
                                <div class="relative">
                                    {avatar(&m.name, "size-8 text-sm")}
                                    <span class=format!(
                                        "absolute -right-0.5 -bottom-0.5 rounded-full border-[3px] size-3.5 border-card {}",
                                        if host { "bg-destructive" } else { "bg-success" }
                                    )></span>
                                </div>
                                <div class="min-w-0">
                                    <div class="text-sm font-medium truncate text-accent-foreground">
                                        {m.name.clone()}
                                        {you.then(|| view! { <span class="ml-1 font-normal text-muted-foreground">"(você)"</span> })}
                                    </div>
                                    {sub.map(|(t, c)| view! { <div class=format!("text-xs truncate {c}")>{t}</div> })}
                                </div>
                            </div>
                        }
                    }).collect_view()
                }}
            </div>

            <div node_ref=list class="overflow-y-auto flex-1 py-3 min-h-0">
                {move || chat.with(|c| c.is_empty()).then(|| view! {
                    <div class="flex flex-col gap-2 items-center px-6 pt-10 text-sm text-center text-muted-foreground">
                        <icons::MessagesSquare class="size-8" />
                        "Ninguém falou nada ainda. Manda um oi!"
                    </div>
                })}
                {move || {
                    let msgs = chat.get();
                    let mut prev: Option<(String, f64)> = None;
                    msgs.into_iter().map(|m| {
                        if m.system {
                            prev = None;
                            return view! {
                                <div class="flex gap-2 items-center py-1 px-4 text-xs text-muted-foreground">
                                    <icons::ArrowRight class="size-3.5 text-success shrink-0" />
                                    <span class="flex-1">{m.text}</span>
                                    <span class="text-[10px]">{chat_time(m.ts)}</span>
                                </div>
                            }.into_any();
                        }
                        // Mensagens seguidas da mesma pessoa ficam agrupadas, como no Discord
                        let grouped = prev.as_ref().is_some_and(|(id, ts)| *id == m.id && m.ts - ts < 5.0 * 60_000.0);
                        prev = Some((m.id.clone(), m.ts));
                        if grouped {
                            view! {
                                <div class="py-0.5 pr-4 pl-[60px] text-sm leading-relaxed break-words select-text hover:bg-black/10 text-foreground">{m.text}</div>
                            }.into_any()
                        } else {
                            view! {
                                <div class="flex gap-3 px-4 pt-3 pb-0.5 hover:bg-black/10">
                                    {avatar(&m.name, "size-8 text-sm mt-0.5")}
                                    <div class="min-w-0">
                                        <div class="flex gap-2 items-baseline">
                                            <span class="text-sm font-medium text-accent-foreground">{m.name.clone()}</span>
                                            <span class="text-[11px] text-muted-foreground">{chat_time(m.ts)}</span>
                                        </div>
                                        <div class="text-sm leading-relaxed break-words select-text text-foreground">{m.text}</div>
                                    </div>
                                </div>
                            }.into_any()
                        }
                    }).collect_view()
                }}
            </div>

            <div class="p-3 pt-0 shrink-0">
                <div class="flex gap-2 items-center pr-1 pl-3 rounded-lg bg-input focus-within:ring-2 focus-within:ring-primary">
                    <input
                        class="flex-1 min-w-0 h-11 text-sm bg-transparent outline-none text-accent-foreground placeholder:text-muted-foreground"
                        placeholder="Conversar na sala"
                        maxlength="500"
                        prop:value=move || draft.get()
                        on:input=move |ev| draft.set(event_target_value(&ev))
                        on:keydown=move |ev| if ev.key() == "Enter" { send() }
                    />
                    <button
                        title="Enviar"
                        class="flex justify-center items-center rounded-md transition-colors size-9 text-muted-foreground hover:text-primary disabled:opacity-40"
                        disabled=move || draft.with(|d| d.trim().is_empty())
                        on:click=move |_| send()
                    >
                        <icons::SendHorizontal class="size-5" />
                    </button>
                </div>
            </div>
        </aside>
    }
}

const ROUND: &str = "flex justify-center items-center rounded-full transition-colors size-12";

/// Botão redondo da barra de controles (ligado = fundo branco)
fn round_button(title: Signal<String>, on: Signal<bool>, on_click: impl Fn() + 'static, icon: AnyView) -> impl IntoView {
    view! {
        <button
            title=move || title.get()
            class=move || format!("{ROUND} {}", if on.get() { "bg-white text-black" } else { "bg-white/10 text-white hover:bg-white/20" })
            on:click=move |_| on_click()
        >
            {icon}
        </button>
    }
}

/// Liga/desliga a tela cheia só da área da transmissão
fn toggle_fullscreen(stage: NodeRef<leptos::html::Div>) {
    let Some(doc) = web_sys::window().and_then(|w| w.document()) else { return };
    if doc.fullscreen_element().is_some() {
        doc.exit_fullscreen();
    } else if let Some(el) = stage.get_untracked() {
        let _ = el.request_fullscreen();
    }
}

#[component]
fn Sharing() -> impl IntoView {
    let ctx = expect_context::<Ctx>();
    let status = ctx.share;
    let show_panel = RwSignal::new(true);
    let stage = NodeRef::<leptos::html::Div>::new();

    // Prévia do que está indo para os amigos (foto da tela/janela a cada 2 s)
    let preview = RwSignal::new(None::<String>);
    let load_preview = move || {
        let Some(src) = ctx.live_source.get_untracked() else { return };
        #[derive(Serialize)]
        struct Args<'a> {
            source: &'a Source,
        }
        let args = to_js(&Args { source: &src.source });
        leptos::task::spawn_local(async move {
            if let Ok(v) = invoke("thumbnail", args).await {
                preview.try_set(v.as_string());
            }
        });
    };
    load_preview();
    if let Ok(timer) = set_interval_with_handle(load_preview, std::time::Duration::from_secs(2)) {
        on_cleanup(move || timer.clear());
    }

    let code = Signal::derive(move || match status.with(|s| s.state.clone()) {
        ShareState::Live { code, .. } => code,
        _ => "······".into(),
    });
    let link = Signal::derive(move || match status.with(|s| s.state.clone()) {
        ShareState::Live { link, .. } => link,
        _ => String::new(),
    });
    let live = move || matches!(status.with(|s| s.state.clone()), ShareState::Live { .. });
    let my_name = move || ctx.settings.with(|s| s.nick.clone());
    let members = Signal::derive(move || {
        let list = status.with(|s| s.members.clone());
        // Antes do servidor responder, mostra pelo menos você
        if list.is_empty() { vec![Member { id: "me".into(), name: my_name(), role: "host".into() }] } else { list }
    });
    let me = Signal::derive(move || {
        members.with(|m| m.iter().find(|m| m.role == "host").map(|m| m.id.clone()).unwrap_or_default())
    });
    let send = Callback::new(move |text: String| {
        #[derive(Serialize)]
        struct Args {
            text: String,
        }
        leptos::task::spawn_local(async move {
            let _ = invoke("send_chat", to_js(&Args { text })).await;
        });
    });

    view! {
        <RoomHeader code=code link=link />
        <div class="flex flex-1 min-h-0">
            <div node_ref=stage class="flex relative flex-col flex-1 min-w-0 bg-black">
                // Topo: ao vivo + estatísticas
                <div class="flex absolute inset-x-0 top-0 z-10 gap-2 items-center p-3">
                    {live_pill()}
                    <span class="py-0.5 px-2 text-xs text-white rounded bg-black/60 tabular-nums">
                        {move || status.with(|s| format!("{:.0} fps · {:.1} Mbps · {:.0} ms", s.fps, s.mbps, s.encode_ms))}
                    </span>
                    {move || {
                        let enc = status.with(|s| s.encoder.clone());
                        let gpu = enc.starts_with("Placa");
                        (!enc.is_empty()).then(|| view! {
                            <span class="flex gap-1.5 items-center py-0.5 px-2 text-xs text-white rounded bg-black/60">
                                {if gpu { view! { <icons::Zap class="size-3" /> }.into_any() } else { view! { <icons::Cpu class="size-3" /> }.into_any() }}
                                {enc}
                            </span>
                        })
                    }}
                </div>

                <div class="flex flex-col flex-1 gap-3 justify-center items-center p-6 min-h-0 text-sm text-muted-foreground">
                    {move || if !live() {
                        view! { <Spinner />"Criando sala…" }.into_any()
                    } else {
                        match preview.get() {
                            Some(url) => view! { <img src=url class="object-contain min-h-0 rounded-md shadow-2xl max-h-full max-w-full" /> }.into_any(),
                            None => view! { <icons::ScreenShare class="size-12" /> }.into_any(),
                        }
                    }}
                    <div class="flex gap-2 items-center text-xs">
                        <span class="rounded-full animate-pulse size-2 bg-destructive"></span>
                        {move || match ctx.live_source.get() {
                            Some(src) => format!("Você está transmitindo: {}", src.label),
                            None => "Você está transmitindo".into(),
                        }}
                    </div>
                </div>

                // Controles embaixo, como numa call do Discord
                <div class="flex gap-3 justify-center items-center pb-6 shrink-0">
                    {round_button(
                        Signal::derive(|| "Copiar convite".to_string()),
                        Signal::derive(|| false),
                        move || { copy_to_clipboard(&link.get_untracked()); show_toast(ctx, "Convite copiado") },
                        view! { <icons::UserPlus class="size-5" /> }.into_any(),
                    )}
                    {round_button(
                        Signal::derive(|| "Tela cheia".to_string()),
                        Signal::derive(|| false),
                        move || toggle_fullscreen(stage),
                        view! { <icons::Maximize class="size-5" /> }.into_any(),
                    )}
                    {round_button(
                        Signal::derive(move || if show_panel.get() { "Esconder chat".into() } else { "Mostrar chat".into() }),
                        show_panel.into(),
                        move || show_panel.update(|v| *v = !*v),
                        view! { <icons::MessageSquare class="size-5" /> }.into_any(),
                    )}
                    <button
                        class="flex gap-2 items-center px-5 h-12 font-semibold text-white rounded-full transition-colors bg-destructive hover:bg-destructive/85"
                        on:click=move |_| stop_sharing(ctx)
                    >
                        <icons::CircleStop class="size-5" />"Parar transmissão"
                    </button>
                </div>
            </div>
            <Show when=move || show_panel.get()>
                <RoomPanel
                    members=members
                    me=me
                    routes=Signal::derive(move || status.with(|s| s.routes.clone()))
                    chat=Signal::derive(move || status.with(|s| s.chat.clone()))
                    on_send=send
                />
            </Show>
        </div>
    }
}

#[component]
fn Watching() -> impl IntoView {
    let ctx = expect_context::<Ctx>();
    let video = NodeRef::<leptos::html::Video>::new();
    let stage = NodeRef::<leptos::html::Div>::new();
    let status = RwSignal::new(ViewerStatus { state: Some("connecting".into()), ..Default::default() });
    let chat = RwSignal::new(Vec::<ChatMsg>::new());
    let muted = RwSignal::new(false);
    let show_panel = RwSignal::new(true);
    let handle = StoredValue::new_local(None::<JsValue>);

    // Inicia quando o <video> existe; para ao sair da tela
    Effect::new(move |_| {
        let Some(el) = video.get() else { return };
        if handle.with_value(|h| h.is_some()) {
            return;
        }
        let on_status = Closure::<dyn FnMut(JsValue)>::new(move |v: JsValue| {
            let Ok(s) = serde_wasm_bindgen::from_value::<ViewerStatus>(v) else { return };
            if let Some(msg) = s.chat {
                chat.try_update(|c| c.push(msg));
                return;
            }
            status.try_update(|cur| {
                if s.members.is_some() {
                    cur.members = s.members;
                }
                if s.me.is_some() {
                    cur.me = s.me;
                }
                let Some(state) = s.state else { return };
                cur.state = Some(state);
                cur.message = s.message.or(cur.message.take());
                if s.fps.is_some() {
                    cur.fps = s.fps;
                    cur.mbps = s.mbps;
                }
                if s.route.is_some() {
                    cur.route = s.route;
                }
            });
        });
        let site = ctx.settings.get_untracked().site;
        let nick = ctx.settings.with_untracked(|s| s.nick.trim().to_string());
        let h = viewer_start(&site, &ctx.code.get_untracked(), &nick, &el, &on_status);
        on_status.forget();
        handle.set_value(Some(h));
    });
    on_cleanup(move || {
        if let Some(h) = handle.try_get_value().flatten() {
            if let Ok(stop) = js_sys::Reflect::get(&h, &"stop".into()) {
                let _ = js_sys::Function::from(stop).call0(&h);
            }
        }
        if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
            if doc.fullscreen_element().is_some() {
                doc.exit_fullscreen();
            }
        }
    });
    let send = Callback::new(move |text: String| {
        let Some(h) = handle.get_value() else { return };
        if let Ok(f) = js_sys::Reflect::get(&h, &"chat".into()) {
            let _ = js_sys::Function::from(f).call1(&h, &text.into());
        }
    });

    let live = move || status.with(|s| s.state.as_deref() == Some("live"));
    let code = ctx.code.get_untracked();
    let link = format!("{}/#/sala/{code}", ctx.settings.with_untracked(|s| s.site.clone()));

    view! {
        <RoomHeader code=code link=link />
        <div class="flex flex-1 min-h-0">
            <div node_ref=stage class="flex relative flex-col flex-1 min-w-0 bg-black group" on:dblclick=move |_| toggle_fullscreen(stage)>
                <video node_ref=video autoplay playsinline prop:muted=move || muted.get() class="object-contain flex-1 w-full min-h-0"></video>

                <Show when=move || !live()>
                    <div class="flex absolute inset-0 flex-col gap-3 justify-center items-center text-sm text-muted-foreground">
                        {move || match status.get().state.unwrap_or_default().as_str() {
                            "error" => view! {
                                <div class="px-6 max-w-md text-center text-foreground">{status.get().message.unwrap_or_default()}</div>
                                <Button variant=ButtonVariant::Secondary on:click=move |_| ctx.page.set(Page::Home)>"Voltar"</Button>
                            }.into_any(),
                            state => view! {
                                <Spinner />
                                <div>{if state == "connecting" { "Conectando…".to_string() } else {
                                    match status.with(|s| s.members.as_ref()?.iter().find(|m| m.role == "host").map(|m| m.name.clone())) {
                                        Some(host) => format!("Esperando {host} voltar a transmitir…"),
                                        None => "Esperando a transmissão começar…".to_string(),
                                    }
                                }}</div>
                            }.into_any(),
                        }}
                    </div>
                </Show>

                // Topo: quem transmite, caminho e estatísticas (some sem o mouse)
                <div class=move || format!(
                    "flex absolute inset-x-0 top-0 gap-2 items-center p-3 bg-gradient-to-b to-transparent transition-opacity duration-300 from-black/70 {}",
                    if live() { "opacity-0 group-hover:opacity-100" } else { "opacity-0" }
                )>
                    {live_pill()}
                    {move || status.with(|s| s.members.as_ref()?.iter().find(|m| m.role == "host").map(|m| m.name.clone())).map(|host| view! {
                        <span class="text-sm font-medium text-white">{host}</span>
                    })}
                    {move || status.with(|s| s.route.clone()).map(|r| {
                        let (text, dot) = route_label(&r);
                        view! {
                            <span class="flex gap-1.5 items-center py-0.5 px-2 text-xs text-white rounded bg-black/50">
                                <span class=format!("rounded-full size-1.5 {dot}")></span>{text}
                            </span>
                        }
                    })}
                    <div class="flex-1"></div>
                    <span class="text-xs text-white/70 tabular-nums">
                        {move || status.with(|s| format!("{:.0} fps · {:.1} Mbps", s.fps.unwrap_or(0.0), s.mbps.unwrap_or(0.0)))}
                    </span>
                </div>

                <div class="flex absolute inset-x-0 bottom-6 gap-3 justify-center items-center opacity-0 transition-opacity duration-300 group-hover:opacity-100">
                    {round_button(
                        Signal::derive(move || if muted.get() { "Ativar som".into() } else { "Silenciar".into() }),
                        muted.into(),
                        move || muted.update(|m| *m = !*m),
                        view! { {move || if muted.get() { view! { <icons::VolumeX class="size-5" /> }.into_any() } else { view! { <icons::Volume2 class="size-5" /> }.into_any() }} }.into_any(),
                    )}
                    {round_button(
                        Signal::derive(|| "Tela cheia".to_string()),
                        Signal::derive(|| false),
                        move || toggle_fullscreen(stage),
                        view! { <icons::Maximize class="size-5" /> }.into_any(),
                    )}
                    {round_button(
                        Signal::derive(move || if show_panel.get() { "Esconder chat".into() } else { "Mostrar chat".into() }),
                        show_panel.into(),
                        move || show_panel.update(|v| *v = !*v),
                        view! { <icons::MessageSquare class="size-5" /> }.into_any(),
                    )}
                    <button title="Sair da sala" class=format!("{ROUND} text-white bg-destructive hover:bg-destructive/85") on:click=move |_| ctx.page.set(Page::Home)>
                        <icons::PhoneOff class="size-5" />
                    </button>
                </div>
            </div>
            <Show when=move || show_panel.get()>
                <RoomPanel
                    members=Signal::derive(move || status.with(|s| s.members.clone().unwrap_or_default()))
                    me=Signal::derive(move || status.with(|s| s.me.clone().unwrap_or_default()))
                    routes=Signal::derive(std::collections::BTreeMap::new)
                    chat=chat
                    on_send=send
                />
            </Show>
        </div>
    }
}

// ── Configurações ────────────────────────────────────────────────────────

#[component]
fn SettingsModal() -> impl IntoView {
    let ctx = expect_context::<Ctx>();
    let mbps = RwSignal::new(ctx.settings.get_untracked().mbps.to_string());
    Effect::new(move |_| {
        let m = mbps.get().parse().unwrap_or(6);
        ctx.settings.update(|cfg| cfg.mbps = m);
    });

    view! {
        <Show when=move || ctx.show_settings.get()>
            <div class="flex fixed inset-0 z-50 justify-center items-center p-4 bg-black/70 animate-in fade-in-0" on:click=move |_| ctx.show_settings.set(false)>
                <div class="overflow-hidden w-full max-w-md rounded-lg shadow-2xl bg-background animate-in zoom-in-95" on:click=|ev| ev.stop_propagation()>
                    <div class="flex justify-between items-start p-5 pb-0">
                        <div>
                            <h2 class="text-xl font-bold text-accent-foreground">"Configurações"</h2>
                            <p class="mt-1 text-sm text-muted-foreground">"Valem para quando você transmite."</p>
                        </div>
                        <button class="transition-colors text-muted-foreground hover:text-accent-foreground" on:click=move |_| ctx.show_settings.set(false)>
                            <icons::X class="size-5" />
                        </button>
                    </div>
                    <div class="flex flex-col gap-6 p-5">
                        <div>
                            <div class="flex justify-between items-baseline">
                                {section_label("Qualidade máxima")}
                                <span class="text-sm font-semibold text-primary tabular-nums">{move || format!("{} Mbps", mbps.get())}</span>
                            </div>
                            <Slider bind_value=mbps min="2" max="20" class="w-full" />
                            <p class="mt-1 text-xs text-muted-foreground">"O app baixa sozinho se a rede apertar. Mais qualidade usa mais internet de quem transmite."</p>
                        </div>
                        <div>
                            {section_label("Padrões da transmissão")}
                            <p class="text-sm text-muted-foreground">"Resolução, quadros, prioridade e som ficam na tela de Transmitir, junto do botão."</p>
                        </div>
                        <div class="flex justify-between items-center pt-4 text-xs border-t text-muted-foreground border-white/5">
                            <span>{concat!("PartyScreen v", env!("CARGO_PKG_VERSION"))}</span>
                            <Button size=ButtonSize::Sm on:click=move |_| ctx.show_settings.set(false)>"Pronto"</Button>
                        </div>
                    </div>
                </div>
            </div>
        </Show>
    }
}

/// Pede o nick antes de transmitir ou entrar numa sala (e para trocar depois)
#[component]
fn NickModal() -> impl IntoView {
    let ctx = expect_context::<Ctx>();
    let draft = RwSignal::new(String::new());
    let input = NodeRef::<leptos::html::Input>::new();
    Effect::new(move |_| {
        if ctx.ask_nick.get().is_some() {
            draft.set(ctx.settings.with_untracked(|s| s.nick.clone()));
            if let Some(el) = input.get() {
                let _ = el.focus();
            }
        }
    });
    let valid = move || draft.with(|d| !d.trim().is_empty());
    let confirm = move || {
        if !valid() {
            return;
        }
        let nick: String = draft.get_untracked().trim().chars().take(24).collect();
        ctx.settings.update(|s| s.nick = nick);
        if let Some(page) = ctx.ask_nick.get_untracked() {
            ctx.page.set(page);
        }
        ctx.ask_nick.set(None);
    };

    view! {
        <Show when=move || ctx.ask_nick.get().is_some()>
            <div class="flex fixed inset-0 z-50 justify-center items-center p-4 bg-black/70 animate-in fade-in-0" on:click=move |_| ctx.ask_nick.set(None)>
                <div class="overflow-hidden w-full max-w-sm text-center rounded-lg shadow-2xl bg-background animate-in zoom-in-95" on:click=|ev| ev.stop_propagation()>
                    <div class="flex flex-col items-center p-6 pb-4">
                        {move || {
                            let name = draft.get();
                            if name.trim().is_empty() {
                                view! { <div class="flex justify-center items-center rounded-full size-16 bg-elevated text-muted-foreground"><icons::Users class="size-7" /></div> }.into_any()
                            } else {
                                avatar(name.trim(), "size-16 text-2xl").into_any()
                            }
                        }}
                        <h2 class="mt-4 text-xl font-bold text-accent-foreground">"Como te chamam?"</h2>
                        <p class="mt-1 text-sm text-muted-foreground">"É assim que a galera vai te ver na sala."</p>
                        <input
                            node_ref=input
                            class="px-3 mt-5 w-full h-10 text-base rounded outline-none bg-input text-accent-foreground placeholder:text-muted-foreground focus:ring-2 focus:ring-primary"
                            placeholder="Seu nick"
                            maxlength="24"
                            prop:value=move || draft.get()
                            on:input=move |ev| draft.set(event_target_value(&ev))
                            on:keydown=move |ev| if ev.key() == "Enter" { confirm() }
                        />
                    </div>
                    <div class="flex gap-2 justify-end py-3 px-4 bg-card">
                        <Button variant=ButtonVariant::Ghost on:click=move |_| ctx.ask_nick.set(None)>"Cancelar"</Button>
                        <Button attr:disabled=move || !valid() on:click=move |_| confirm()>"Continuar"</Button>
                    </div>
                </div>
            </div>
        </Show>
    }
}

#[component]
fn Toast() -> impl IntoView {
    let ctx = expect_context::<Ctx>();
    view! {
        {move || ctx.toast.get().map(|text| view! {
            <div class="fixed bottom-6 left-1/2 z-50 py-2.5 px-4 text-sm font-medium rounded-md shadow-xl -translate-x-1/2 bg-popover text-accent-foreground animate-in fade-in-0 slide-in-from-bottom-2">
                <span class="inline-flex gap-2 items-center"><icons::Check class="size-4 text-success" />{text}</span>
            </div>
        })}
    }
}
