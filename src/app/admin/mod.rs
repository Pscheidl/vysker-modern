#[cfg(not(feature = "demo"))]
mod accounts;
#[cfg(not(feature = "demo"))]
mod api;
#[cfg(not(feature = "demo"))]
mod editor;
#[cfg(not(feature = "demo"))]
mod events;
#[cfg(not(feature = "demo"))]
mod lists;
#[cfg(not(feature = "demo"))]
mod markdown_editor;
#[cfg(not(feature = "demo"))]
mod navigation;
#[cfg(not(feature = "demo"))]
mod recovery;
#[cfg(not(feature = "demo"))]
mod subscribers;
#[cfg(not(feature = "demo"))]
mod ui;

#[cfg(feature = "demo")]
use leptos::prelude::*;

#[cfg(feature = "demo")]
#[component]
pub fn Administration() -> impl IntoView {
    view! { <div class="page-width interior"><super::components::PageHeading title="Administrace" description="Administrace je dostupná v serverové verzi webu. Tento náhled používá ukázková data."/><a class="button secondary" href=super::site_url("/")>"Zpět na web"</a></div> }
}

#[cfg(not(feature = "demo"))]
pub use workspace::*;

#[cfg(not(feature = "demo"))]
mod workspace {
    use super::{
        accounts::Accounts,
        api::{self, Kind, Session},
        editor::Editor,
        lists::{AuditLog, Entries, MailHistory, Overview},
        ui::*,
    };
    use crate::app::components::{Icon, Logo, ThemeSwitch};
    use leptos::prelude::*;
    use leptos_meta::Title;
    use leptos_router::hooks::use_location;
    use serde_json::json;

    #[derive(Clone)]
    pub enum Access {
        Checking,
        SignedOut,
        Ready(Session),
        Failed(api::Failure),
    }
    #[derive(Clone, Copy)]
    pub struct AdminContext {
        pub access: RwSignal<Access>,
        pub dirty: RwSignal<bool>,
        pub flash: RwSignal<Option<String>>,
        pub board: Option<crate::app::BoardResource>,
    }
    impl AdminContext {
        pub fn changed(self, message: &str) {
            self.flash.set(Some(message.into()));
            if let Some(board) = self.board {
                board.refetch();
            }
        }
        pub fn leave(self) -> bool {
            if !self.dirty.get_untracked() {
                return true;
            }
            #[cfg(feature = "hydrate")]
            {
                window()
                    .confirm_with_message("Máte neuložené změny. Opravdu chcete odejít?")
                    .unwrap_or(false)
            }
            #[cfg(not(feature = "hydrate"))]
            {
                false
            }
        }
    }
    #[component]
    pub fn Administration() -> impl IntoView {
        let context = AdminContext {
            access: RwSignal::new(Access::Checking),
            dirty: RwSignal::new(false),
            flash: RwSignal::new(None),
            board: use_context::<crate::app::BoardResource>(),
        };
        provide_context(context);
        Effect::new(move |_| {
            leptos::task::spawn_local(async move {
                let result = api::get::<Session>("/api/v1/admin/session").await;
                context.access.set(match result {
                    Ok(session) => Access::Ready(session),
                    Err(error) if error.status == 401 => Access::SignedOut,
                    Err(error) => Access::Failed(error),
                });
            });
        });
        #[cfg(feature = "hydrate")]
        {
            let listener = window_event_listener(leptos::ev::beforeunload, move |ev| {
                if context.dirty.get_untracked() {
                    ev.prevent_default();
                    ev.set_return_value("");
                }
            });
            on_cleanup(move || listener.remove());
        }
        let location = use_location();
        view! {<Title text="Správa webu · Vyskeř"/>
            {move || match context.access.get() {
                Access::Checking=>view!{<div class="admin-login"><Pending/></div>}.into_any(),
                Access::SignedOut=>view!{<SignedOut/>}.into_any(),
                Access::Failed(error)=>view!{<div class="admin-login"><div class="admin-login-card"><FailureView error/><a href="/admin" class="button secondary">"Zkusit znovu"</a></div></div>}.into_any(),
                Access::Ready(_) if location.pathname.get()=="/admin/obnova"=>view!{<super::recovery::Recovery/>}.into_any(),
                Access::Ready(session)=>view!{<AdminShell email=session.email/>}.into_any(),
            }}
            <noscript><p class="admin-noscript">"Pro správu webu zapněte JavaScript."</p></noscript>
        }
    }
    #[component]
    fn SignedOut() -> impl IntoView {
        let location = use_location();
        view! {{move || if location.pathname.get()=="/admin/obnova" {
            view!{<super::recovery::Recovery/>}.into_any()
        } else { view!{<Login/>}.into_any() }}}
    }
    #[component]
    fn Login() -> impl IntoView {
        let ctx = expect_context::<AdminContext>();
        let email = RwSignal::new(String::new());
        let password = RwSignal::new(String::new());
        let error = RwSignal::new(None);
        let login = Action::new_local(move |_: &()| {
            let input = json!({"email":email.get_untracked(),"password":password.get_untracked()});
            async move {
                error.set(None);
                match api::request::<Session>("POST", "/api/v1/admin/login", Some(input), None)
                    .await
                {
                    Ok(session) => {
                        password.set(String::new());
                        ctx.access.set(Access::Ready(session));
                    }
                    Err(failure) => {
                        password.set(String::new());
                        error.set(Some(failure));
                    }
                }
            }
        });
        view! {
            <div class="admin-login">
                <div class="admin-login-art" aria-hidden="true"><Logo/><span>"Vyskeř"</span><p>"Místo pro všechno důležité."</p><div class="admin-login-terrain"></div></div>
                <div class="admin-login-card"><a href="/" class="back-link"><Icon name="back"/>"Zpět na web obce"</a>
                    <p class="eyebrow">"SPRÁVA WEBU"</p><h1>"Dobrý den."</h1><p>"Přihlaste se ke správě obecního webu."</p>
                    <form on:submit=move |ev|{ev.prevent_default(); if !login.pending().get_untracked(){login.dispatch(());}}>
                        <fieldset disabled=move || login.pending().get()>
                            <label class="admin-field" for="admin-email"><span>"E-mail"</span><input id="admin-email" type="email" autocomplete="username" required maxlength="254" prop:value=move || email.get() on:input=move |ev|email.set(event_target_value(&ev))/></label>
                            <label class="admin-field" for="admin-password"><span>"Heslo"</span><input id="admin-password" type="password" autocomplete="current-password" required maxlength="1024" prop:value=move || password.get() on:input=move |ev|password.set(event_target_value(&ev))/></label>
                            <ErrorMessage error/>
                            <button class="button primary" type="submit">{move || if login.pending().get(){"Přihlašuji…"}else{"Přihlásit se"}}<Icon/></button>
                        </fieldset>
                    </form>
                    <a href="/admin/obnova" class="back-link">"Zapomenuté heslo"</a>
                    <p class="admin-caption">"Přístup pro pověřené správce obce. Potřebujete účet? Obraťte se na správce webu."</p><ThemeSwitch/>
                </div>
            </div>
        }
    }
    #[component]
    fn AdminShell(email: String) -> impl IntoView {
        let ctx = expect_context::<AdminContext>();
        let location = use_location();
        let logout_error = RwSignal::new(None);
        let logout = Action::new_local(move |_: &()| async move {
            match api::save("DELETE", "/api/v1/admin/session", None).await {
                Ok(_) => {
                    ctx.dirty.set(false);
                    ctx.flash.set(None);
                    ctx.access.set(Access::SignedOut);
                }
                Err(error) if error.status == 401 => {
                    ctx.dirty.set(false);
                    ctx.access.set(Access::SignedOut);
                }
                Err(error) => logout_error.set(Some(error)),
            }
        });
        Effect::new(move |_| {
            let _ = location.pathname.get();
            ctx.dirty.set(false);
        });
        view! {
            <div class="admin-workspace">
                <aside class="admin-rail"><a href="/" target="_blank" rel="noopener" class="brand"><Logo/><span>"Vyskeř"</span></a><p class="eyebrow">"SPRÁVA OBCE"</p>
                    <nav aria-label="Administrace">
                        <AdminLink href="/admin" label="Přehled" icon="grid" exact=true/>
                        <AdminLink href="/admin/uredni-deska" label="Úřední deska" icon="board"/>
                        <AdminLink href="/admin/dokumenty" label="Dokumenty" icon="paper"/>
                        <AdminLink href="/admin/stranky" label="Stránky" icon="pages"/>
                        <AdminLink href="/admin/kalendar" label="Kalendář" icon="calendar"/>
                        <AdminLink href="/admin/odberatele" label="Odběratelé" icon="mail"/>
                        <AdminLink href="/admin/navigace" label="Navigace" icon="pages"/>
                        <AdminLink href="/admin/kategorie" label="Kategorie" icon="grid"/>
                        <AdminLink href="/admin/posta" label="Rozesílání" icon="mail"/>
                        <AdminLink href="/admin/ucty" label="Účty a hesla" icon="grid"/>
                        <AdminLink href="/admin/audit" label="Audit" icon="history"/>
                    </nav>
                    <div class="admin-rail-bottom"><a href="/" target="_blank" rel="noopener"><Icon name="external"/>"Otevřít web"</a><ThemeSwitch/><div class="admin-user"><span class="admin-avatar">"S"</span><div><strong>"Správce obce"</strong><span>{email}</span></div></div>
                        <button class="admin-signout" type="button" disabled=move || logout.pending().get() on:click=move |_|{if ctx.leave(){logout.dispatch(());}}><Icon name="logout"/>"Odhlásit se"</button>
                    </div>
                </aside>
                <div class="admin-main"><ErrorMessage error=logout_error/>
                    {move ||ctx.flash.get().map(|message|view!{<div class="admin-flash" role="status"><Icon name="check"/><span>{message}</span><button type="button" aria-label="Zavřít zprávu" on:click=move |_|ctx.flash.set(None)><Icon name="close"/></button></div>})}
                    {move || {
                        let path=location.pathname.get();
                        let tail=path.trim_start_matches("/admin").trim_matches('/');
                        match tail {
                            ""=>view!{<Overview/>}.into_any(),
                            "uredni-deska"=>view!{<Entries kind=Kind::Notice/>}.into_any(),
                            "dokumenty"=>view!{<Entries kind=Kind::Document/>}.into_any(),
                            "stranky"=>view!{<Entries kind=Kind::Page/>}.into_any(),
                            "kalendar"=>view!{<super::events::Events/>}.into_any(),
                            "odberatele"=>view!{<super::subscribers::Subscribers/>}.into_any(),
                            "navigace"=>view!{<super::navigation::NavigationSettings/>}.into_any(),
                            "kategorie"=>view!{<super::navigation::NavigationSettings categories=true/>}.into_any(),
                            "posta"=>view!{<MailHistory/>}.into_any(),
                            "ucty"=>view!{<Accounts/>}.into_any(),
                            "audit"=>view!{<AuditLog/>}.into_any(),
                            _=> {
                                let (section,id)=tail.split_once('/').unwrap_or(("",""));
                                if section=="kalendar" {
                                    return if id=="nove"{view!{<super::events::EventEditor id=0/>}.into_any()}
                                    else if let Ok(id)=id.parse::<i64>(){view!{<super::events::EventEditor id/>}.into_any()}
                                    else{view!{<h1>"Akce neexistuje"</h1>}.into_any()};
                                }
                                let kind=match section {"uredni-deska"=>Some(Kind::Notice),"dokumenty"=>Some(Kind::Document),"stranky"=>Some(Kind::Page),_=>None};
                                match (kind, id) {
                                    (Some(kind),"nove")=>view!{<Editor kind id=0/>}.into_any(),
                                    (Some(kind),id) if id.parse::<i64>().is_ok_and(|v|v>0)=>view!{<Editor kind id={id.parse::<i64>().unwrap_or(0)}/>}.into_any(),
                                    _=>view!{<h1>"Stránka neexistuje"</h1><AdminLink href="/admin" label="Zpět na přehled"/>}.into_any(),
                                }
                            }
                        }
                    }}
                </div>
            </div>
        }
    }
}
