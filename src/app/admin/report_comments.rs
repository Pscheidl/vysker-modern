use super::{Access, AdminContext, api, ui::*};
use crate::reports::{REPORT_NAME_LIMIT, REPORT_UPLOAD_BYTES, ReportComment};
use leptos::prelude::*;
use serde_json::json;

#[derive(Clone)]
struct CommentPhoto {
    file: web_sys::File,
    preview: String,
}

fn failure(message: &str) -> api::Failure {
    api::Failure {
        status: 0,
        message: message.into(),
    }
}

fn release_preview(url: &str) {
    #[cfg(feature = "hydrate")]
    let _ = web_sys::Url::revoke_object_url(url);
    #[cfg(not(feature = "hydrate"))]
    let _ = url;
}

fn confirm(message: &str) -> bool {
    #[cfg(feature = "hydrate")]
    {
        web_sys::window()
            .is_some_and(|window| window.confirm_with_message(message).unwrap_or(false))
    }
    #[cfg(not(feature = "hydrate"))]
    {
        let _ = message;
        false
    }
}

fn default_name(ctx: AdminContext) -> String {
    match ctx.access.get_untracked() {
        Access::Ready(session) => session.display_name.unwrap_or_default(),
        _ => String::new(),
    }
}

async fn save_comment(
    report_id: i64,
    editing: Option<ReportComment>,
    author_name: String,
    comment: String,
    removed: Vec<i64>,
    photos: Vec<CommentPhoto>,
) -> api::Result<ReportComment> {
    #[cfg(feature = "hydrate")]
    {
        let session: api::Session = api::get("/api/v1/admin/session").await?;
        let network = || {
            failure(
                "Spojení se serverem se přerušilo. Text i fotografie zůstávají ve formuláři. Před dalším odesláním obnovte seznam komentářů a ověřte, zda už byl komentář uložen.",
            )
        };
        let data = web_sys::FormData::new().map_err(|_| network())?;
        data.append_with_str("author_name", &author_name)
            .map_err(|_| network())?;
        data.append_with_str("body", &comment)
            .map_err(|_| network())?;
        if let Some(original) = &editing {
            data.append_with_str("expected_updated_at", &original.updated_at)
                .map_err(|_| network())?;
            data.append_with_str(
                "remove_photo_ids",
                &serde_json::to_string(&removed).map_err(|_| network())?,
            )
            .map_err(|_| network())?;
        }
        for photo in photos {
            data.append_with_blob_and_filename("photo", &photo.file, &photo.file.name())
                .map_err(|_| network())?;
        }
        let request = match editing {
            Some(original) => gloo_net::http::Request::put(&format!(
                "/api/v1/admin/reports/{report_id}/comments/{}",
                original.id
            )),
            None => gloo_net::http::Request::post(&format!(
                "/api/v1/admin/reports/{report_id}/comments"
            )),
        };
        let response = request
            .credentials(web_sys::RequestCredentials::SameOrigin)
            .header("X-CSRF-Token", &session.csrf_token)
            .body(data)
            .map_err(|_| network())?
            .send()
            .await
            .map_err(|_| network())?;
        let status = response.status();
        let value = response
            .json::<serde_json::Value>()
            .await
            .map_err(|_| network())?;
        if !response.ok() {
            return Err(api::Failure {
                status,
                message: value
                    .get("error")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or(
                        "Komentář se nepodařilo uložit. Zkontrolujte údaje a zkuste to znovu.",
                    )
                    .into(),
            });
        }
        serde_json::from_value(value).map_err(|_| {
            failure(
                "Server vrátil neočekávanou odpověď. Obnovte seznam komentářů a ověřte uložení.",
            )
        })
    }
    #[cfg(not(feature = "hydrate"))]
    {
        let _ = (report_id, editing, author_name, comment, removed, photos);
        Err(failure("Komentář lze uložit po načtení formuláře."))
    }
}

#[component]
pub fn CommentThread(
    report_id: i64,
    hidden: Signal<bool>,
    dirty: RwSignal<bool>,
    updating: RwSignal<bool>,
) -> impl IntoView {
    let ctx = expect_context::<AdminContext>();
    let revision = RwSignal::new(0_u64);
    let comments = LocalResource::new(move || {
        revision.get();
        async move {
            api::get::<Vec<ReportComment>>(&format!("/api/v1/admin/reports/{report_id}/comments"))
                .await
        }
    });
    let editing = RwSignal::new(None::<ReportComment>);
    let initial_name = RwSignal::new(default_name(ctx));
    let author_name = RwSignal::new(initial_name.get_untracked());
    let body = RwSignal::new(String::new());
    let removed = RwSignal::new(Vec::<i64>::new());
    let photos = RwSignal::new_local(Vec::<CommentPhoto>::new());
    let input = NodeRef::<leptos::html::Input>::new();
    let author_input = NodeRef::<leptos::html::Input>::new();
    let error = RwSignal::new(None::<api::Failure>);
    let changed = Memo::new(move |_| {
        let original = editing.get();
        let original_body = original
            .as_ref()
            .map(|item| item.body.as_str())
            .unwrap_or("");
        author_name.get() != initial_name.get()
            || body.get() != original_body
            || !removed.get().is_empty()
            || !photos.with(Vec::is_empty)
    });
    Effect::new(move |_| dirty.set(changed.get()));
    let reset = move || {
        for photo in photos.get_untracked() {
            release_preview(&photo.preview);
        }
        photos.set(Vec::new());
        removed.set(Vec::new());
        editing.set(None);
        let name = default_name(ctx);
        initial_name.set(name.clone());
        author_name.set(name);
        body.set(String::new());
        error.set(None);
    };
    on_cleanup(move || {
        for photo in photos.get_untracked() {
            release_preview(&photo.preview);
        }
        dirty.set(false);
    });
    let save = Action::new_local(move |_: &()| {
        let original = editing.get_untracked();
        let author_value = author_name.get_untracked();
        let body_value = body.get_untracked();
        let removed_value = removed.get_untracked();
        let photos_value = photos.get_untracked();
        async move {
            updating.set(true);
            error.set(None);
            let result = save_comment(
                report_id,
                original,
                author_value,
                body_value,
                removed_value,
                photos_value,
            )
            .await;
            if updating.try_set(false).is_some() {
                return;
            }
            match result {
                Ok(_) => {
                    reset();
                    revision.update(|value| *value += 1);
                    ctx.changed("Komentář byl uložen.");
                }
                Err(failure) => error.set(Some(failure)),
            }
        }
    });
    let delete_error = RwSignal::new(None::<api::Failure>);
    let delete = Action::new_local(move |comment: &ReportComment| {
        let id = comment.id;
        let expected_updated_at = comment.updated_at.clone();
        async move {
            updating.set(true);
            delete_error.set(None);
            let result = api::save(
                "DELETE",
                &format!("/api/v1/admin/reports/{report_id}/comments/{id}"),
                Some(json!({ "expected_updated_at": expected_updated_at })),
            )
            .await;
            if updating.try_set(false).is_some() {
                return;
            }
            match result {
                Ok(_) => {
                    if editing.get_untracked().is_some_and(|item| item.id == id) {
                        reset();
                    }
                    revision.update(|value| *value += 1);
                    ctx.changed("Komentář byl smazán včetně fotografií.");
                }
                Err(failure) => delete_error.set(Some(failure)),
            }
        }
    });
    view! {
        <section class="admin-form-section report-comments" aria-label="Komentáře obce">
            <div class="admin-panel-heading">
                <h3>"Komentáře obce"</h3>
                <button class="button secondary" type="button" disabled=move || updating.get() on:click=move |_| {
                    delete_error.set(None);
                    comments.refetch();
                }>"Obnovit komentáře"</button>
            </div>
            <p class="admin-caption">{move || if hidden.get() {
                "Hlášení je skryté. Komentáře a fotografie uvidí veřejnost až po jeho opětovném zveřejnění."
            } else {
                "Komentáře, jméno autora a přiložené fotografie se zobrazují ve veřejném detailu hlášení."
            }}</p>
            <ErrorMessage error=delete_error/>
            <Suspense fallback=Pending>{move || comments.get().map(|result| match result {
                Err(error) => view! { <FailureView error/> }.into_any(),
                Ok(items) => view! {
                    {(items.is_empty()).then(|| view! { <p class="admin-empty">"Zatím bez komentářů."</p> })}
                    <div class="report-comments-list">{items.into_iter().map(|comment| {
                        let comment_id = comment.id;
                        let edit_comment = comment.clone();
                        let delete_comment = comment.clone();
                        let edited = comment.updated_at != comment.created_at;
                        view! {
                            <article class="admin-report-comment report-comment" aria-label=format!("Komentář od {}", comment.author_name)>
                                <header class="report-comment-header"><strong>{comment.author_name.clone()}</strong><time datetime=comment.created_at.clone()>{crate::events::date_label(&comment.created_at)}</time>
                                    {edited.then(|| view! { <span class="admin-caption">"Upraveno "<time datetime=comment.updated_at.clone()>{crate::events::date_label(&comment.updated_at)}</time></span> })}
                                </header>
                                <p class="report-comment-body" style="white-space: pre-wrap; overflow-wrap: anywhere">{comment.body}</p>
                                <div class="report-detail-photos">{comment.photo_ids.into_iter().enumerate().map(move |(index, photo_id)| {
                                    let url = format!("/api/v1/admin/reports/{report_id}/comments/{comment_id}/photos/{photo_id}");
                                    view! { <a class="report-detail-photo" href=url.clone() target="_blank" rel="noopener noreferrer" aria-label=format!("Otevřít fotografii {} komentáře {comment_id}", index + 1)><img src=url.clone() alt=format!("Fotografie {} ke komentáři", index + 1) loading="lazy"/></a> }
                                }).collect_view()}</div>
                                <div class="admin-actions">
                                    <button class="button secondary" type="button" disabled=move || updating.get() on:click=move |_| {
                                        if changed.get_untracked() && !confirm("Komentář obsahuje neuložené změny. Opravdu je chcete zahodit?") { return; }
                                        reset();
                                        let original = edit_comment.clone();
                                        initial_name.set(original.author_name.clone());
                                        author_name.set(original.author_name.clone());
                                        body.set(original.body.clone());
                                        editing.set(Some(original));
                                        if let Some(element) = author_input.get() { let _ = element.focus(); }
                                    }>"Upravit komentář"</button>
                                    <button class="button secondary danger" type="button" disabled=move || updating.get() on:click=move |_| {
                                        if confirm("Opravdu chcete smazat tento komentář včetně všech jeho fotografií? Tuto akci nelze vrátit.") { delete.dispatch(delete_comment.clone()); }
                                    }>"Smazat komentář"</button>
                                </div>
                            </article>
                        }
                    }).collect_view()}</div>
                }.into_any(),
            })}</Suspense>
            <form class="report-comment-form" on:submit=move |event| {
                event.prevent_default();
                if !updating.get_untracked() && !author_name.get_untracked().trim().is_empty() && !body.get_untracked().trim().is_empty() && changed.get_untracked() {
                    save.dispatch(());
                }
            }>
                <h4>{move || if editing.get().is_some() { "Upravit komentář" } else { "Nový komentář" }}</h4>
                <fieldset disabled=move || updating.get()>
                    <label class="admin-field" for="report-comment-author"><span>"Jméno autora"</span><input node_ref=author_input id="report-comment-author" type="text" required maxlength=REPORT_NAME_LIMIT autocomplete="name" aria-describedby="report-comment-author-help" prop:value=move || author_name.get() on:input=move |event| author_name.set(event_target_value(&event))/></label>
                    <p id="report-comment-author-help" class="admin-caption">"Jméno bude uvedeno u komentáře. "<a class="text-link" href="/admin/ucty" on:click=move |event| { if !ctx.leave() { event.prevent_default(); } }>"Výchozí jméno si nastavíte v účtu."</a>" Bez nastaveného jména ho vyplňte u každého nového komentáře."</p>
                    <label class="admin-field" for="report-comment-body"><span>"Text komentáře"</span><textarea id="report-comment-body" rows="5" required maxlength="5000" prop:value=move || body.get() on:input=move |event| body.set(event_target_value(&event)) placeholder="Popište aktuální postup, odpověď nebo výsledek řešení."></textarea></label>
                    <div class="report-detail-photos" aria-label="Uložené fotografie komentáře">{move || {
                        let removed_ids = removed.get();
                        editing.get().map(|original| original.photo_ids.into_iter().filter(|photo_id| !removed_ids.contains(photo_id)).map(move |photo_id| {
                            let url = format!("/api/v1/admin/reports/{report_id}/comments/{}/photos/{photo_id}", original.id);
                            view! { <figure class="report-photo-preview-item"><a class="report-detail-photo" href=url.clone() target="_blank" rel="noopener noreferrer"><img src=url.clone() alt="Uložená fotografie komentáře" loading="lazy"/></a><button class="text-link" type="button" aria-label=format!("Odebrat uloženou fotografii komentáře {photo_id}") on:click=move |_| removed.update(|items| items.push(photo_id))>"Odebrat fotografii"</button></figure> }
                        }).collect_view())
                    }}</div>
                    <Show when=move || !removed.get().is_empty()><button class="text-link" type="button" on:click=move |_| removed.set(Vec::new())>"Vrátit odebrané fotografie komentáře"</button><p class="admin-caption">"Fotografie budou odebrány až po uložení komentáře."</p></Show>
                    <label class="admin-field" for="report-comment-photos"><span>"Fotografie komentáře (nepovinné)"</span><input node_ref=input id="report-comment-photos" type="file" multiple accept="image/jpeg,image/png,image/webp" aria-describedby="report-comment-photo-help" on:change=move |_| {
                        let Some(element) = input.get() else { return; };
                        let Some(files) = element.files() else { return; };
                        let selected: Vec<web_sys::File> = (0..files.length()).filter_map(|index| files.item(index)).collect();
                        element.set_value("");
                        error.set(None);
                        let invalid_type = selected.iter().any(|file| {
                            let kind = file.type_();
                            let name = file.name().to_lowercase();
                            !matches!(kind.as_str(), "image/jpeg" | "image/png" | "image/webp")
                                && !(kind.is_empty() && [".jpg", ".jpeg", ".png", ".webp"].iter().any(|extension| name.ends_with(extension)))
                        });
                        let total = photos.with(|items| items.iter().map(|item| item.file.size()).sum::<f64>()) + selected.iter().map(|file| file.size()).sum::<f64>();
                        if invalid_type || total > REPORT_UPLOAD_BYTES as f64 {
                            error.set(Some(failure(if invalid_type { "Vyberte fotografie JPG, PNG nebo WebP." } else { "Nové fotografie dohromady přesahují 100 MB." })));
                            return;
                        }
                        #[cfg(feature = "hydrate")]
                        for file in selected {
                            match web_sys::Url::create_object_url_with_blob(&file) {
                                Ok(preview) => photos.update(|items| items.push(CommentPhoto { file, preview })),
                                Err(_) => error.set(Some(failure("Náhled fotografie se nepodařilo načíst. Zkuste soubor vybrat znovu."))),
                            }
                        }
                    }/></label>
                    <p id="report-comment-photo-help" class="admin-caption">"JPG, PNG nebo WebP, nejvýše 100 MB nových fotografií celkem. Přidávat je můžete postupně. Uložené fotografie včetně nových mohou mít dohromady nejvýše 100 MB."</p>
                    <div class="report-photo-previews" aria-label="Nové fotografie komentáře">{move || photos.get().into_iter().enumerate().map(|(index, photo)| {
                        let name = photo.file.name();
                        view! { <figure class="report-photo-preview-item" data-comment-photo=name.clone()><img src=photo.preview alt=format!("Náhled fotografie komentáře {name}")/><figcaption>{name.clone()}</figcaption><button class="text-link" type="button" aria-label=format!("Odebrat novou fotografii komentáře {name}") on:click=move |_| photos.update(|items| {
                            if index < items.len() { let photo = items.remove(index); release_preview(&photo.preview); }
                        })>"Odebrat fotografii"</button></figure> }
                    }).collect_view()}</div>
                    <ErrorMessage error/>
                    <div class="admin-actions">
                        <button class="button primary" type="submit" disabled=move || !changed.get() || author_name.get().trim().is_empty() || body.get().trim().is_empty()>{move || if save.pending().get() { "Ukládám komentář…" } else if editing.get().is_some() { "Uložit změny komentáře" } else { "Zveřejnit komentář" }}</button>
                        <button class="button secondary" type="button" disabled=move || !changed.get() && editing.get().is_none() on:click=move |_| {
                            if !changed.get_untracked() || confirm("Komentář obsahuje neuložené změny. Opravdu je chcete zahodit?") { reset(); }
                        }>{move || if editing.get().is_some() { "Zrušit úpravy komentáře" } else { "Vymazat rozepsaný komentář" }}</button>
                    </div>
                </fieldset>
            </form>
        </section>
    }
}
