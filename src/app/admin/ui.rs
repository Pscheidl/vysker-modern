use super::{AdminContext, api};
use crate::app::components::Icon;
use leptos::prelude::*;
use leptos_router::components::A;

#[component]
pub fn Pending() -> impl IntoView {
    view! {<div class="admin-pending" role="status"><span class="loading-dot"></span>"Načítám…"</div>}
}
#[component]
pub fn AdminLink(
    #[prop(into)] href: String,
    #[prop(into)] label: String,
    #[prop(default = "arrow")] icon: &'static str,
    #[prop(default = false)] exact: bool,
) -> impl IntoView {
    let ctx = expect_context::<AdminContext>();
    view! {<A href exact attr:class="admin-link" on:click=move |ev: leptos::ev::MouseEvent|{if !ev.ctrl_key() && !ev.meta_key() && !ctx.leave(){ev.prevent_default();}}><Icon name=icon/><span>{label}</span></A>}
}
#[component]
pub fn FailureView(error: api::Failure) -> impl IntoView {
    view! {<div class="admin-error" role="alert"><Icon name="info"/><div><p>{error.message}</p>{(error.status==401).then(||view!{<a href="/admin" target="_blank" rel="noopener">"Přihlásit se v novém okně"</a><p>"Po přihlášení zde akci zopakujte."</p>})}</div></div>}
}
#[component]
pub fn ErrorMessage(error: RwSignal<Option<api::Failure>>) -> impl IntoView {
    view! {{move ||error.get().map(|error|view!{<FailureView error/>})}}
}
#[component]
pub fn Status(#[prop(into)] value: String) -> impl IntoView {
    let class = format!("admin-status state-{value}");
    view! {<span class=class><span></span>{api::state_label(&value)}</span>}
}
#[component]
pub fn Heading(#[prop(into)] title: String, #[prop(into)] description: String) -> impl IntoView {
    view! {<header class="admin-heading"><p class="eyebrow">"OBEC VYSKEŘ"</p><h1>{title}</h1><p>{description}</p></header>}
}
#[component]
pub fn Field(
    id: &'static str,
    label: &'static str,
    value: RwSignal<String>,
    #[prop(default = "text")] kind: &'static str,
    #[prop(default = false)] required: bool,
    #[prop(default = false)] readonly: bool,
    #[prop(default = "1000")] maxlength: &'static str,
    #[prop(default = "")] help: &'static str,
) -> impl IntoView {
    let ctx = expect_context::<AdminContext>();
    let hint = format!("{id}-hint");
    view! {<label class="admin-field" for=id><span>{label}{required.then_some(" *")}</span><input id=id type=kind required=required readonly=readonly maxlength=maxlength prop:value=move ||value.get() aria-describedby=hint.clone() on:input=move |ev|{value.set(event_target_value(&ev));ctx.dirty.set(true);}/><small id=hint>{help}</small></label>}
}
#[component]
pub fn TextArea(
    id: &'static str,
    label: &'static str,
    value: RwSignal<String>,
    #[prop(default = false)] required: bool,
    #[prop(default = "50000")] maxlength: &'static str,
    #[prop(default = "6")] rows: &'static str,
) -> impl IntoView {
    let ctx = expect_context::<AdminContext>();
    view! {<label class="admin-field" for=id><span>{label}{required.then_some(" *")}</span><textarea id=id required=required maxlength=maxlength rows=rows prop:value=move ||value.get() on:input=move |ev|{value.set(event_target_value(&ev));ctx.dirty.set(true);}></textarea></label>}
}
#[component]
pub fn ConfirmButton(
    label: &'static str,
    #[prop(into)] title: String,
    #[prop(into)] description: String,
    #[prop(into)] disabled: Signal<bool>,
    on_confirm: Callback<()>,
    #[prop(default = false)] danger: bool,
) -> impl IntoView {
    let dialog = NodeRef::<leptos::html::Dialog>::new();
    let class = if danger {
        "button danger"
    } else {
        "button primary"
    };
    view! {
        <button type="button" class=class disabled=move ||disabled.get() on:click=move |_|{if let Some(dialog)=dialog.get(){let _=dialog.show_modal();}}>{label}</button>
        <dialog node_ref=dialog class="admin-dialog" aria-label=title.clone()>
            <h2>{title.clone()}</h2><p>{description}</p><div class="admin-dialog-actions">
                <button type="button" class="button secondary" autofocus on:click=move |_|{if let Some(dialog)=dialog.get(){dialog.close();}}>"Zrušit"</button>
                <button type="button" class=class disabled=move ||disabled.get() on:click=move |_|{if let Some(dialog)=dialog.get(){dialog.close();}on_confirm.run(());}>{label}</button>
            </div>
        </dialog>
    }
}
#[component]
pub fn Pager(offset: RwSignal<usize>, count: usize) -> impl IntoView {
    view! {<nav class="admin-pager" aria-label="Stránkování"><span>{format!("Strana {}",offset.get_untracked()/20+1)}</span><div><button type="button" class="button secondary" disabled=move ||offset.get()==0 on:click=move |_|offset.update(|v|*v=v.saturating_sub(20))>"Předchozí"</button><button type="button" class="button secondary" disabled=count<=20 on:click=move |_|offset.update(|v|*v+=20)>"Další"</button></div></nav>}
}
