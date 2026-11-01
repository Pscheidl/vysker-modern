use super::{Access, AdminContext, api, rentals::refresh_rental_permissions, ui::*};
use leptos::prelude::*;
use leptos_meta::Title;
use serde::Deserialize;
use serde_json::json;

#[derive(Clone, Copy, Debug, Deserialize)]
struct RentalSettings {
    enabled: bool,
    version: i64,
}

#[component]
pub fn RentalBoundary(
    #[prop(default = false)] overview: bool,
    children: ChildrenFn,
) -> impl IntoView {
    let ctx = expect_context::<AdminContext>();
    let data = LocalResource::new(move || async move {
        let can_manage = refresh_rental_permissions(ctx).await?;
        let settings = api::get::<RentalSettings>("/api/v1/admin/rental-settings").await?;
        Ok::<_, api::Failure>((settings, can_manage))
    });
    view! {
        <Suspense fallback=Pending>{move || data.get().map(|result| match result {
            Ok((settings, can_manage)) => view! {
                <RentalBoundaryContent settings can_manage overview children=children.clone()/>
            }.into_any(),
            Err(error) => view! {
                <div class="rental-admin">
                    <Heading title="Pronájmy" description="Nastavení rezervačního systému se nepodařilo načíst."/>
                    <FailureView error/>
                    <button class="button secondary" type="button" on:click=move |_| data.refetch()>"Zkusit znovu"</button>
                </div>
            }.into_any(),
        })}</Suspense>
    }
}

#[component]
fn RentalBoundaryContent(
    settings: RentalSettings,
    can_manage: bool,
    overview: bool,
    children: ChildrenFn,
) -> impl IntoView {
    let ctx = expect_context::<AdminContext>();
    let site = use_context::<crate::app::SiteResource>();
    let current = RwSignal::new(settings);
    let allowed = RwSignal::new(can_manage);
    let error = RwSignal::new(None::<api::Failure>);
    let save = Action::new_local(move |enabled: &bool| {
        let enabled = *enabled;
        let input = json!({
            "enabled": enabled,
            "expected_version": current.get_untracked().version,
        });
        async move {
            let _ = error.try_set(None);
            match api::save("PUT", "/api/v1/admin/rental-settings", Some(input)).await {
                Ok(value) => match serde_json::from_value::<RentalSettings>(value) {
                    Ok(settings) => {
                        let _ = current.try_set(settings);
                        if let Some(site) = site {
                            site.refetch();
                        }
                        let _ = ctx.flash.try_set(Some(if settings.enabled {
                            "Rezervační systém je zapnutý.".into()
                        } else {
                            "Rezervační systém je vypnutý. Uložená data zůstala zachovaná.".into()
                        }));
                    }
                    Err(_) => {
                        let _ = error.try_set(Some(api::Failure {
                            status: 0,
                            message: "Odpověď na změnu nelze přečíst. Obnovte stránku a zkontrolujte aktuální stav.".into(),
                        }));
                    }
                },
                Err(failure) => {
                    if failure.status == 401 {
                        let _ = ctx.access.try_set(Access::SignedOut);
                    } else if matches!(failure.status, 403 | 409) {
                        // Refresh a stale version before allowing another explicit confirmation.
                        if let Ok(settings) =
                            api::get::<RentalSettings>("/api/v1/admin/rental-settings").await
                        {
                            let _ = current.try_set(settings);
                            if let Some(site) = site {
                                site.refetch();
                            }
                        }
                        if failure.status == 403 {
                            if let Ok(can_manage) = refresh_rental_permissions(ctx).await {
                                let _ = allowed.try_set(can_manage);
                            }
                        }
                    }
                    let _ = error.try_set(Some(failure));
                }
            }
        }
    });
    view! {
        {overview.then(|| view! {
            <Title text="Pronájmy · Správa Vyskře"/>
            <div class="rental-admin">
                <Heading title="Pronájmy" description="Provoz systému, žádosti o pronájem a vyúčtování obecního majetku."/>
                <section class="admin-panel rental-system-settings" aria-label="Provoz rezervačního systému">
                    <div class="rental-system-heading">
                        <div><span class="rental-admin-kicker">"PROVOZ REZERVACÍ"</span><h2>"Rezervační systém"</h2></div>
                        <span class="rental-system-status" class:enabled=move || current.get().enabled role="status">{move || if current.get().enabled {"Zapnuto"} else {"Vypnuto"}}</span>
                    </div>
                    <p>{move || if current.get().enabled {
                        "Veřejnost může podávat žádosti. Správa pronájmů, upomínky a automatická vyúčtování jsou v provozu."
                    } else {
                        "Rezervace nejsou veřejně dostupné. Správa pronájmů, upomínky a automatická vyúčtování jsou pozastavené. Uložená data zůstávají zachovaná."
                    }}</p>
                    <Show when=move || allowed.get() fallback=|| view! {<p class="admin-caption">"Stav může změnit pouze pověřený správce rezervací."</p>}>
                        <div class="rental-system-actions">
                            {move || if current.get().enabled {
                                view! {<ConfirmButton label="Vypnout rezervace" title="Vypnout rezervační systém?" description="Rezervace zmizí z veřejné nabídky. Pozastaví se přístup k rezervacím, jejich správa, upomínky i automatická vyúčtování. Uložená data zůstanou zachovaná a systém lze později znovu zapnout." disabled=Signal::derive(move || save.pending().get()) on_confirm=Callback::new(move |_| {save.dispatch(false);}) danger=true/>}.into_any()
                            } else {
                                view! {<ConfirmButton label="Zapnout rezervace" title="Zapnout rezervační systém?" description="Rezervace se zpřístupní veřejnosti i správcům. Obnoví se upomínky a automatická vyúčtování včetně dřívějších nevyřízených pronájmů. Zapněte systém až po schválení podmínek pronájmu." disabled=Signal::derive(move || save.pending().get()) on_confirm=Callback::new(move |_| {save.dispatch(true);})/>}.into_any()
                            }}
                            <Show when=move || save.pending().get()><span class="admin-caption" role="status">"Ukládám změnu…"</span></Show>
                        </div>
                    </Show>
                    <ErrorMessage error/>
                </section>
            </div>
        })}
        <Show when=move || current.get().enabled fallback=move || view! {
            <div class="rental-admin rental-system-disabled">
                {if overview {
                    view! {<h2>"Rezervační systém je vypnutý"</h2><p>"Přehled pronájmů, majetek a platební údaje budou dostupné po jeho zapnutí."</p>}.into_any()
                } else {
                    view! {
                        <Title text="Pronájmy jsou vypnuté · Správa Vyskře"/>
                        <Heading title="Rezervační systém je vypnutý" description="Správa pronájmů je pozastavená. Uložená data zůstávají zachovaná."/>
                        <AdminLink href="/admin/pronajmy" label="Přejít na nastavení rezervací" icon="back"/>
                    }.into_any()
                }}
            </div>
        }>{children()}</Show>
    }
}
