use leptos::prelude::*;
use tw_merge::*;

#[derive(Default, Clone, Copy, PartialEq, Eq, strum::AsRefStr, strum::Display)]
pub enum SliderVariant {
    #[default]
    Round,
    Flat,
}

// Adaptado do Rust/UI: valor ligado a um sinal (`bind_value`) e limites configuráveis.
#[component]
pub fn Slider(
    bind_value: RwSignal<String>,
    #[prop(into)] min: String,
    #[prop(into)] max: String,
    #[prop(optional, into)] class: String,
    #[prop(default = SliderVariant::default())] variant: SliderVariant,
) -> impl IntoView {
    let variant_attr = variant.to_string();
    let merged_class = tw_merge!(
        "overflow-hidden relative bg-transparent transition-all duration-100 ease-in-out appearance-none disabled:opacity-30 disabled:cursor-not-allowed text-[1.5rem] w-[12.5em] text-primary active:cursor-grabbing disabled:grayscale",
        class
    );

    view! { <input data-name="Slider" data-variant=variant_attr type="range" min=min max=max bind:value=bind_value class=merged_class /> }
}
