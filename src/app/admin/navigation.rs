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
    parent_id: Option<i64>,
    sort_order: i64,
    visible: bool,
    version: i64,
    usage_count: i64,
}

#[derive(Clone)]
struct EditorSelection {
    key: u64,
    item: Item,
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
    let selected = RwSignal::new(None::<EditorSelection>);
    let busy = RwSignal::new(false);
    let editor_revision = RwSignal::new(0u64);
    let error = RwSignal::new(None);
    let data = LocalResource::new(move || {
        let _ = reload.get();
        async move { api::get::<Vec<Item>>(&format!("/api/v1/admin/{endpoint}")).await }
    });
    let open_editor = Callback::new(move |item: Item| {
        if !busy.get_untracked() && ctx.leave() {
            ctx.dirty.set(false);
            editor_revision.update(|revision| *revision += 1);
            selected.set(Some(EditorSelection {
                key: editor_revision.get_untracked(),
                item,
            }));
        }
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
                    ctx.dirty.set(false);
                    selected.set(None);
                    ctx.changed("Položka byla odstraněna.");
                    if let Some(site) = site {
                        site.refetch();
                    }
                    reload.update(|v| *v += 1);
                }
                Err(e) => error.set(Some(e)),
            }
            busy.set(false);
        }
    });
    view! {
        <Heading title=if categories { "Kategorie" } else { "Navigace" }
            description=if categories {
                "Názvy a pořadí kategorií úřední desky. Používané kategorie nelze odstranit."
            } else {
                "Uspořádejte menu webu. Pod hlavní položku můžete přidat jednu úroveň podpoložek. Nižší pořadí se v každé skupině zobrazuje dříve."
            }/>
        <ErrorMessage error/>
        <div class="admin-navigation-toolbar">
            <button class="button primary" type="button" disabled=move || busy.get() || data.get().is_none() on:click=move |_| {
                open_editor.run(Item { visible: true, sort_order: 100, ..Default::default() });
            }>"Přidat položku"</button>
        </div>
        <For
            each=move || { selected.get().into_iter().collect::<Vec<_>>() }
            key=|selection| selection.key
            children=move |selection| {
                let items = data.get_untracked().and_then(Result::ok).unwrap_or_default();
                view! { <ItemEditor categories item=selection.item items selected reload busy/> }
            }
        />
        <Suspense fallback=Pending>{move || data.get().map(|result| match result {
            Err(error) => view! { <FailureView error/> }.into_any(),
            Ok(mut items) => {
                items.sort_by_key(|item| (item.sort_order, item.id));
                if categories {
                    view! {
                        <div class="admin-table-wrap"><table class="admin-table">
                            <thead><tr><th>"Pořadí"</th><th>"Název"</th><th>"Použití"</th><th>"Akce"</th></tr></thead>
                            <tbody>{items.into_iter().map(|item| {
                                let edit = item.clone();
                                let delete = item.clone();
                                view! {
                                    <tr><td>{item.sort_order}</td><td>{item.name}</td><td>{format!("{} záznamů", item.usage_count)}</td>
                                        <td><div class="admin-actions">
                                            <button class="button secondary" type="button" disabled=move || busy.get() on:click=move |_| open_editor.run(edit.clone())>"Upravit"</button>
                                            <button class="button secondary" type="button" disabled=move || { busy.get() || item.usage_count > 0 } on:click=move |_| {
                                                #[cfg(feature="hydrate")]
                                                if !busy.get_untracked() && ctx.leave() && window().confirm_with_message("Opravdu odstranit tuto položku?").unwrap_or(false) { busy.set(true); remove.dispatch(delete.clone()); }
                                                #[cfg(not(feature="hydrate"))] let _ = &delete;
                                            }>"Odstranit"</button>
                                        </div></td>
                                    </tr>
                                }
                            }).collect_view()}</tbody>
                        </table></div>
                    }.into_any()
                } else if items.is_empty() {
                    view! { <div class="admin-panel admin-empty"><h2>"Menu zatím nemá žádné položky"</h2><p>"Začněte přidáním hlavní položky. Potom pod ni můžete zařadit další odkazy."</p></div> }.into_any()
                } else {
                    let groups = items.iter().filter(|item| item.parent_id.is_none()).map(|root| {
                        let children = items.iter().filter(|item| item.parent_id == Some(root.id)).cloned().collect::<Vec<_>>();
                        (root.clone(), children)
                    }).collect::<Vec<_>>();
                    view! {
                        <ul class="admin-navigation-tree" aria-label="Struktura menu">
                            {groups.into_iter().map(|(root, children)| {
                                let hidden_parent = !root.visible;
                                let child_count = children.len();
                                let children_label = format!("Podpoložky: {}", root.label);
                                view! {
                                    <li class="admin-panel admin-navigation-group">
                                        <NavigationRow item=root child_count hidden_parent=false open_editor remove busy/>
                                        {(!children.is_empty()).then(|| view! {
                                            <ul class="admin-navigation-children" aria-label=children_label>
                                                {children.into_iter().map(|item| view! {
                                                    <li><NavigationRow item child_count=0 hidden_parent open_editor remove busy/></li>
                                                }).collect_view()}
                                            </ul>
                                        })}
                                    </li>
                                }
                            }).collect_view()}
                        </ul>
                    }.into_any()
                }
            }
        })}</Suspense>
    }
}

#[component]
fn NavigationRow(
    item: Item,
    child_count: usize,
    hidden_parent: bool,
    open_editor: Callback<Item>,
    remove: Action<Item, ()>,
    busy: RwSignal<bool>,
) -> impl IntoView {
    let ctx = expect_context::<AdminContext>();
    let is_root = item.parent_id.is_none();
    let edit = item.clone();
    let delete = item.clone();
    let id = item.id;
    let delete_hint = format!("navigation-delete-help-{id}");
    let status = if !item.visible {
        "Skryto"
    } else if hidden_parent {
        "Skryto s hlavní položkou"
    } else {
        "Zobrazeno"
    };
    let status_class = if item.visible && !hidden_parent {
        "admin-status state-published"
    } else {
        "admin-status state-draft"
    };
    let title = format!("Upravit položku {}", item.label);
    let delete_title = format!("Odstranit položku {}", item.label);
    let add_title = format!("Přidat podpoložku pod {}", item.label);
    view! {
        <div class="admin-navigation-row">
            <div class="admin-navigation-info">
                <div class="admin-navigation-name"><strong>{item.label}</strong><span class=status_class><span></span>{status}</span></div>
                <span class="admin-navigation-path">{item.path}</span>
                <span class="admin-navigation-meta">{if is_root { "Hlavní položka" } else { "Podpoložka" }}" · Pořadí "{item.sort_order}</span>
            </div>
            <div class="admin-navigation-controls">
                <div class="admin-actions">
                    <button class="button secondary" type="button" aria-label=title disabled=move || busy.get() on:click=move |_| open_editor.run(edit.clone())>"Upravit"</button>
                    {is_root.then(move || view! {
                        <button class="button secondary" type="button" aria-label=add_title disabled=move || busy.get() on:click=move |_| {
                            open_editor.run(Item { parent_id: Some(id), visible: true, sort_order: 100, ..Default::default() });
                        }>"Přidat podpoložku"</button>
                    })}
                    <button class="button secondary" type="button" aria-label=delete_title
                        aria-describedby={if child_count > 0 { Some(delete_hint.clone()) } else { None }}
                        disabled=move || { busy.get() || child_count > 0 }
                        on:click=move |_| {
                            #[cfg(feature="hydrate")]
                            if !busy.get_untracked() && ctx.leave() && window().confirm_with_message(&format!("Opravdu odstranit položku „{}“?", delete.label)).unwrap_or(false) { busy.set(true); remove.dispatch(delete.clone()); }
                            #[cfg(not(feature="hydrate"))] let _ = (&delete, ctx, remove);
                        }>"Odstranit"</button>
                </div>
                {(child_count > 0).then(|| view! {
                    <small id=delete_hint>"Před odstraněním přesuňte nebo odstraňte podpoložky."</small>
                })}
            </div>
        </div>
    }
}

#[component]
fn ItemEditor(
    categories: bool,
    item: Item,
    items: Vec<Item>,
    selected: RwSignal<Option<EditorSelection>>,
    reload: RwSignal<u64>,
    busy: RwSignal<bool>,
) -> impl IntoView {
    let ctx = expect_context::<AdminContext>();
    let site = use_context::<crate::app::SiteResource>();
    let id = item.id;
    let version = item.version;
    let name = RwSignal::new(if categories { item.name } else { item.label });
    let path = RwSignal::new(item.path);
    let parent_id = RwSignal::new(item.parent_id);
    let order = RwSignal::new(item.sort_order.to_string());
    let visible = RwSignal::new(item.visible);
    let error = RwSignal::new(None);
    let has_children = items.iter().any(|other| other.parent_id == Some(id));
    let mut parents = items
        .into_iter()
        .filter(|other| other.parent_id.is_none() && other.id != id)
        .collect::<Vec<_>>();
    parents.sort_by_key(|parent| (parent.sort_order, parent.id));
    let parent_options = parents.clone();
    let hidden_parent = Memo::new(move |_| {
        parent_id.get().is_some_and(|id| {
            parents
                .iter()
                .any(|parent| parent.id == id && !parent.visible)
        })
    });
    let name_input = NodeRef::<leptos::html::Input>::new();
    #[cfg(feature = "hydrate")]
    Effect::new(move |_| {
        if let Some(input) = name_input.get() {
            let _ = input.focus();
        }
    });
    let save = Action::new_local(move |_: &()| {
        let payload = if categories {
            json!({"name":name.get_untracked(),"sort_order":order.get_untracked().parse::<i64>().unwrap_or(-1),"expected_version":version})
        } else {
            json!({"label":name.get_untracked(),"path":path.get_untracked(),"parent_id":parent_id.get_untracked(),"sort_order":order.get_untracked().parse::<i64>().unwrap_or(-1),"visible":visible.get_untracked(),"expected_version":version})
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
            busy.set(false);
        }
    });
    view! {
        <section class="admin-panel admin-form-section admin-navigation-editor" aria-labelledby="navigation-editor-heading">
            <h2 id="navigation-editor-heading">{if id != 0 { "Úprava položky" } else if item.parent_id.is_some() { "Nová podpoložka" } else { "Nová položka" }}</h2>
            <form on:submit=move |ev| { ev.prevent_default(); if !busy.get_untracked() { busy.set(true); save.dispatch(()); } }>
                <fieldset disabled=move || busy.get()>
                    <label class="admin-field" for="setting-name"><span>"Název *"</span>
                        <input node_ref=name_input id="setting-name" type="text" required maxlength=if categories { "80" } else { "40" }
                            prop:value=move || name.get() on:input=move |ev| { name.set(event_target_value(&ev)); ctx.dirty.set(true); }/>
                    </label>
                    {(!categories).then(move || view! {
                        <label class="admin-field" for="setting-parent"><span>"Umístění v menu"</span>
                            <select id="setting-parent" disabled=has_children aria-describedby="setting-parent-help"
                                prop:value=move || parent_id.get().map(|id| id.to_string()).unwrap_or_default()
                                on:change=move |ev| { parent_id.set(event_target_value(&ev).parse().ok()); ctx.dirty.set(true); }>
                                <option value="">"Hlavní menu"</option>
                                {parent_options.into_iter().map(|parent| view! {
                                    <option value=parent.id.to_string()>{format!("{}{}", parent.label, if parent.visible { "" } else { " (skrytá)" })}</option>
                                }).collect_view()}
                            </select>
                            <small id="setting-parent-help">{if has_children {
                                "Tato položka má podpoložky. Než ji přesunete pod jinou položku, přesuňte nebo odstraňte její podpoložky."
                            } else {
                                "Zvolte hlavní menu nebo položku, pod kterou se má odkaz zobrazit. Podmenu může mít jednu úroveň."
                            }}</small>
                        </label>
                        <PageLinkField path name/>
                        <label class="admin-check"><input type="checkbox" prop:checked=move || visible.get() on:change=move |ev| { visible.set(event_target_checked(&ev)); ctx.dirty.set(true); }/>
                            <span><strong>"Zobrazit v menu"</strong><small>"Skrytím hlavní položky se skryjí i její podpoložky. Samotné stránky zůstávají dostupné."</small></span>
                        </label>
                        <Show when=move || hidden_parent.get()>
                            <p class="admin-navigation-parent-note" role="status">"Hlavní položka je skrytá. Tato podpoložka se v menu neobjeví, dokud nezviditelníte také hlavní položku."</p>
                        </Show>
                    })}
                    <label class="admin-field" for="setting-order"><span>"Pořadí (0 až 9999) *"</span>
                        <input id="setting-order" type="number" min="0" max="9999" step="1" required aria-describedby="setting-order-help"
                            prop:value=move || order.get() on:input=move |ev| { order.set(event_target_value(&ev)); ctx.dirty.set(true); }/>
                        <small id="setting-order-help">{if categories { "Nižší číslo se zobrazuje dříve." } else { "Nižší číslo se zobrazuje dříve v dané skupině. Podpoložky se řadí pouze pod svou hlavní položkou." }}</small>
                    </label>
                    <ErrorMessage error/>
                    <div class="admin-actions">
                        <button class="button primary" type="submit">{move || if save.pending().get() { "Ukládám…" } else { "Uložit položku" }}</button>
                        <button class="button secondary" type="button" on:click=move |_| { if ctx.leave() { ctx.dirty.set(false); selected.set(None); } }>"Zavřít editor"</button>
                    </div>
                </fieldset>
            </form>
        </section>
    }
}

#[component]
fn PageLinkField(path: RwSignal<String>, name: RwSignal<String>) -> impl IntoView {
    let ctx = expect_context::<AdminContext>();
    // A fresh resource for each editor avoids suggesting pages hidden since the app loaded.
    let pages = LocalResource::new(crate::catalog::load_site_info);
    let open = RwSignal::new(false);
    let active = RwSignal::new(None::<usize>);
    let matches = Memo::new(move |_| {
        let query = crate::content::fold(path.get().trim());
        pages
            .get()
            .and_then(Result::ok)
            .map(|info| {
                info.pages
                    .into_iter()
                    .map(|page| (page.title, crate::catalog::page_path(&page.slug)))
                    .filter(|(title, url)| {
                        crate::content::fold(title).contains(&query) || url.contains(&query)
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    });
    let suggestions = Memo::new(move |_| matches.get().into_iter().take(8).collect::<Vec<_>>());
    let expanded = Memo::new(move |_| open.get() && !suggestions.get().is_empty());
    let choose = Callback::new(move |index: usize| {
        if let Some((title, url)) = suggestions.get_untracked().get(index) {
            path.set(url.clone());
            if name.get_untracked().trim().is_empty() {
                name.set(title.chars().take(40).collect());
            }
            ctx.dirty.set(true);
            open.set(false);
            active.set(None);
        }
    });
    view! {
        <div class="admin-field admin-navigation-link-field">
            <label for="setting-path">"Adresa odkazu *"</label>
            <div class="admin-navigation-combobox">
                <input id="setting-path" type="text" required maxlength="100" autocomplete="off"
                    placeholder="Název stránky nebo /adresa" role="combobox" aria-autocomplete="list"
                    aria-controls="navigation-page-options" aria-expanded=move || expanded.get().to_string()
                    aria-activedescendant=move || {
                        expanded.get().then(|| active.get()).flatten().map(|index| format!("navigation-page-option-{index}"))
                    }
                    aria-describedby="setting-path-help navigation-page-status"
                    prop:value=move || path.get()
                    on:focus=move |_| { open.set(true); active.set(None); }
                    on:blur=move |_| { open.set(false); active.set(None); }
                    on:input=move |ev| {
                        path.set(event_target_value(&ev));
                        ctx.dirty.set(true);
                        open.set(true);
                        active.set(None);
                    }
                    on:keydown=move |ev| {
                        let count = suggestions.get_untracked().len();
                        match ev.key().as_str() {
                            "ArrowDown" | "ArrowUp" if count > 0 => {
                                ev.prevent_default();
                                let down = ev.key() == "ArrowDown";
                                let next = if open.get_untracked() {
                                    match active.get_untracked() {
                                        Some(index) if down => (index + 1) % count,
                                        Some(index) => (index + count - 1) % count,
                                        None if down => 0,
                                        None => count - 1,
                                    }
                                } else if down { 0 } else { count - 1 };
                                open.set(true);
                                active.set(Some(next));
                            }
                            "Enter" if expanded.get_untracked() => {
                                // Enter in an open picker must not accidentally submit the form.
                                ev.prevent_default();
                                if let Some(index) = active.get_untracked() { choose.run(index); }
                            }
                            "Escape" if open.get_untracked() => {
                                ev.prevent_default();
                                ev.stop_propagation();
                                open.set(false);
                                active.set(None);
                            }
                            _ => {}
                        }
                    }
                />
                <Show when=move || expanded.get()>
                    <ul id="navigation-page-options" class="admin-navigation-suggestions" role="listbox" aria-label="Zveřejněné stránky">
                        {move || suggestions.get().into_iter().enumerate().map(|(index, (title, url))| {
                            let option_ref = NodeRef::<leptos::html::Li>::new();
                            #[cfg(feature = "hydrate")]
                            Effect::new(move |_| {
                                if active.get() == Some(index) {
                                    if let Some(option) = option_ref.get() { option.scroll_into_view_with_bool(false); }
                                }
                            });
                            view! {
                                <li node_ref=option_ref id=format!("navigation-page-option-{index}") role="option"
                                    aria-selected=move || (active.get() == Some(index)).to_string()
                                    on:mousedown=move |ev| ev.prevent_default()
                                    on:click=move |_| choose.run(index)>
                                    <strong>{title}</strong><span>{url}</span>
                                </li>
                            }
                        }).collect_view()}
                    </ul>
                </Show>
            </div>
            <small id="navigation-page-status" role="status">{move || {
                match pages.get() {
                    None => "Načítám zveřejněné stránky…".to_owned(),
                    Some(Err(_)) => "Nabídku stránek se nepodařilo načíst. Adresu můžete zadat ručně.".to_owned(),
                    Some(Ok(_)) if matches.get().is_empty() => "Žádná zveřejněná stránka neodpovídá. Adresu můžete zadat ručně.".to_owned(),
                    Some(Ok(_)) if matches.get().len() > 8 => "Zobrazeno prvních 8 stránek. Upřesněte název nebo adresu.".to_owned(),
                    Some(Ok(_)) => String::new(),
                }
            }}</small>
            <small id="setting-path-help">"Vyberte zveřejněnou stránku podle názvu nebo adresy, i bez diakritiky. Výběr doplní také prázdný název položky (nejvýše 40 znaků). Adresu můžete zadat i ručně, například /uredni-deska nebo /obec."</small>
        </div>
    }
}
