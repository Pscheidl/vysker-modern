use super::{AdminContext, api, ui::*};
use leptos::prelude::*;
use serde::Deserialize;
use serde_json::json;

#[derive(Clone, Default, Deserialize)]
#[serde(default)]
struct Item {
    id: i64,
    label: String,
    name: String,
    path: String,
    sort_order: i64,
    visible: bool,
    version: i64,
    usage_count: i64,
}

#[component]
pub fn NavigationSettings(#[prop(default = false)] categories: bool) -> impl IntoView {
    let ctx = expect_context::<AdminContext>();
    let site = use_context::<crate::app::SiteResource>();
    let endpoint = if categories {
        "categories"
    } else {
        "navigation"
    };
    let reload = RwSignal::new(0u64);
    let selected = RwSignal::new(None::<Item>);
    let error = RwSignal::new(None);
    let data = LocalResource::new(move || {
        let _ = reload.get();
        async move { api::get::<Vec<Item>>(&format!("/api/v1/admin/{endpoint}")).await }
    });
    let remove = Action::new_local(move |item: &Item| {
        let item = item.clone();
        async move {
            error.set(None);
            match api::save(
                "DELETE",
                &format!("/api/v1/admin/{endpoint}/{}", item.id),
                Some(json!({"expected_version":item.version})),
            )
            .await
            {
                Ok(_) => {
                    ctx.changed("Položka byla odstraněna.");
                    if let Some(site) = site {
                        site.refetch();
                    }
                    reload.update(|v| *v += 1);
                }
                Err(e) => error.set(Some(e)),
            }
        }
    });
    view! {<Heading title=if categories{"Kategorie"}else{"Navigace"} description=if categories{"Názvy a pořadí kategorií úřední desky. Používané kategorie nelze odstranit."}else{"Položky hlavního menu. Nižší pořadí se zobrazuje dříve. Odkazy mohou vést na zveřejněné stránky tohoto webu."}/>
        <ErrorMessage error/><button class="button primary" type="button" on:click=move |_|{if ctx.leave(){selected.set(Some(Item{visible:true,sort_order:100,..Default::default()}));ctx.dirty.set(false);}}>"Přidat položku"</button>
        {move||selected.get().map(|item|view!{<ItemEditor categories item selected reload/>})}
        <Suspense fallback=Pending>{move||data.get().map(|result|match result {
            Err(error)=>view!{<FailureView error/>}.into_any(),
            Ok(items)=>view!{<div class="admin-table-wrap"><table class="admin-table"><thead><tr><th>"Pořadí"</th><th>"Název"</th><th>{if categories{"Použití"}else{"Adresa / viditelnost"}}</th><th>"Akce"</th></tr></thead><tbody>{items.into_iter().map(|item|{
                let edit=item.clone();let delete=item.clone();let label=if categories{item.name.clone()}else{item.label.clone()};
                view!{<tr><td>{item.sort_order}</td><td>{label}</td><td>{if categories{format!("{} záznamů",item.usage_count)}else{format!("{} · {}",item.path,if item.visible{"Zobrazeno"}else{"Skryto"})}}</td><td><div class="admin-actions"><button class="button secondary" type="button" on:click=move |_|{if ctx.leave(){selected.set(Some(edit.clone()));ctx.dirty.set(false);}}>"Upravit"</button><button class="button secondary" type="button" disabled=move||remove.pending().get()||(categories&&item.usage_count>0) on:click=move |_|{
                    #[cfg(feature="hydrate")]
                    if ctx.leave() && window().confirm_with_message("Opravdu odstranit tuto položku?").unwrap_or(false){remove.dispatch(delete.clone());}
                    #[cfg(not(feature="hydrate"))] let _=&delete;
                }>"Odstranit"</button></div></td></tr>}
            }).collect_view()}</tbody></table></div>}.into_any()
        })}</Suspense>
    }
}
#[component]
fn ItemEditor(
    categories: bool,
    item: Item,
    selected: RwSignal<Option<Item>>,
    reload: RwSignal<u64>,
) -> impl IntoView {
    let ctx = expect_context::<AdminContext>();
    let site = use_context::<crate::app::SiteResource>();
    let id = item.id;
    let version = item.version;
    let name = RwSignal::new(if categories { item.name } else { item.label });
    let path = RwSignal::new(item.path);
    let order = RwSignal::new(item.sort_order.to_string());
    let visible = RwSignal::new(item.visible);
    let error = RwSignal::new(None);
    let save = Action::new_local(move |_: &()| {
        let payload = if categories {
            json!({"name":name.get_untracked(),"sort_order":order.get_untracked().parse::<i64>().unwrap_or(-1),"expected_version":version})
        } else {
            json!({"label":name.get_untracked(),"path":path.get_untracked(),"sort_order":order.get_untracked().parse::<i64>().unwrap_or(-1),"visible":visible.get_untracked(),"expected_version":version})
        };
        async move {
            error.set(None);
            let endpoint = if categories {
                "categories"
            } else {
                "navigation"
            };
            let endpoint = if id == 0 {
                format!("/api/v1/admin/{endpoint}")
            } else {
                format!("/api/v1/admin/{endpoint}/{id}")
            };
            match api::save(
                if id == 0 { "POST" } else { "PUT" },
                &endpoint,
                Some(payload),
            )
            .await
            {
                Ok(_) => {
                    ctx.dirty.set(false);
                    ctx.changed("Položka byla uložena.");
                    if let Some(site) = site {
                        site.refetch();
                    }
                    selected.set(None);
                    reload.update(|v| *v += 1);
                }
                Err(e) => error.set(Some(e)),
            }
        }
    });
    view! {<section class="admin-panel admin-form-section"><h2>{if id==0{"Nová položka"}else{"Úprava položky"}}</h2><form on:submit=move |ev|{ev.prevent_default();if !save.pending().get_untracked(){save.dispatch(());}}><fieldset disabled=move||save.pending().get()>
        <Field id="setting-name" label="Název" value=name required=true maxlength=if categories{"80"}else{"40"}/>
        {(!categories).then(move||view!{<Field id="setting-path" label="Adresa odkazu" value=path required=true maxlength="100" help="Například /uredni-deska, /kalendar nebo /stranky/spolky. Stránka musí být zveřejněná."/><label class="admin-check"><input type="checkbox" prop:checked=move||visible.get() on:change=move|ev|{visible.set(event_target_checked(&ev));ctx.dirty.set(true);}/><span>"Zobrazit v menu"</span></label>})}
        <Field id="setting-order" label="Pořadí (0 až 9999)" kind="number" value=order required=true/>
        <ErrorMessage error/><div class="admin-actions"><button class="button primary" type="submit">"Uložit položku"</button><button class="button secondary" type="button" on:click=move |_|{if ctx.leave(){ctx.dirty.set(false);selected.set(None);}}>"Zavřít editor"</button></div>
    </fieldset></form></section>}
}
