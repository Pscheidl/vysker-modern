use super::{AdminContext, api, ui::*};
use leptos::prelude::*;

#[derive(Clone, serde::Deserialize)]
struct PageImage {
    id: i64,
    name: String,
    size_bytes: i64,
    width: i32,
    height: i32,
}

// Escape Markdown syntax so an image description stays literal text.
fn image_markdown(alt: &str, id: i64) -> String {
    let escaped: String = alt
        .trim()
        .chars()
        .flat_map(|c| {
            if c.is_ascii_punctuation() {
                vec!['\\', c]
            } else {
                vec![c]
            }
        })
        .collect();
    format!("\n\n![{escaped}](/api/v1/page-images/{id})\n\n")
}

#[component]
pub fn MarkdownEditor(
    id: i64,
    title: RwSignal<String>,
    content: RwSignal<String>,
    busy: Signal<bool>,
    working: RwSignal<bool>,
) -> impl IntoView {
    let ctx = expect_context::<AdminContext>();
    let textarea = NodeRef::<leptos::html::Textarea>::new();
    let input = NodeRef::<leptos::html::Input>::new();
    let preview = RwSignal::new(false);
    let alt = RwSignal::new(String::new());
    let selected = RwSignal::new(false);
    let error = RwSignal::new(None);
    let images = LocalResource::new(move || async move {
        if id == 0 {
            Ok(vec![])
        } else {
            api::get::<Vec<PageImage>>(&format!("/api/v1/admin/pages/{id}/images")).await
        }
    });
    let insert = Callback::new(
        move |(prefix, suffix, placeholder): (String, String, String)| {
            if let Some(el) = textarea.get() {
                // Browser selection offsets use UTF-16, including Czech text and emoji.
                let start = el.selection_start().ok().flatten().unwrap_or(0);
                let end = el.selection_end().ok().flatten().unwrap_or(start);
                let value: Vec<u16> = el.value().encode_utf16().collect();
                let selection = value.get(start as usize..end as usize).unwrap_or(&[]);
                let text = if selection.is_empty() {
                    placeholder
                } else {
                    String::from_utf16_lossy(selection)
                };
                let replacement = format!("{prefix}{text}{suffix}");
                let new_len = value.len() - selection.len() + replacement.encode_utf16().count();
                if new_len > 100_000 {
                    error.set(Some(api::Failure {
                        status: 400,
                        message: "Obsah stránky překračuje limit 100 000 znaků.".into(),
                    }));
                    return;
                }
                if el
                    .set_range_text_with_start_and_end_and_mode(&replacement, start, end, "end")
                    .is_ok()
                {
                    content.set(el.value());
                    ctx.dirty.set(true);
                    let _ = el.focus();
                }
            }
        },
    );
    let upload = Action::new_local(move |file: &web_sys::File| {
        let file = file.clone();
        let description = alt.get_untracked();
        async move {
            error.set(None);
            let result = api::upload(&format!("/api/v1/admin/pages/{id}/images"), file).await;
            working.set(false);
            match result {
                Ok(value) => match serde_json::from_value::<PageImage>(value) {
                    Ok(image) => {
                        // Inserting an image does not replace selected prose.
                        if let Some(el) = textarea.get() {
                            let end = el.selection_end().ok().flatten().unwrap_or(0);
                            let _ = el.set_selection_range(end, end);
                        }
                        insert.run((
                            image_markdown(&description, image.id),
                            String::new(),
                            String::new(),
                        ));
                        images.refetch();
                        selected.set(false);
                        if let Some(el) = input.get() {
                            el.set_value("");
                        }
                        ctx.changed("Obrázek je nahraný. Uložte stránku, aby se vložil i do zveřejněného textu.");
                    }
                    Err(_) => error.set(Some(api::Failure {
                        status: 0,
                        message: "Server vrátil neočekávanou odpověď.".into(),
                    })),
                },
                Err(failure) => error.set(Some(failure)),
            }
        }
    });
    view! {
        <div class="markdown-editor">
            <div class="markdown-toolbar" role="group" aria-label="Formátování Markdown">
                {[
                    ("Nadpis", "\n\n## ", "\n\n", "Nadpis"),
                    ("Tučně", "**", "**", "tučný text"),
                    ("Kurzíva", "*", "*", "text kurzívou"),
                    ("Seznam", "\n\n- ", "\n", "položka"),
                    ("Odkaz", "[", "](/kontakt)", "text odkazu"),
                    ("Citace", "\n\n> ", "\n\n", "citovaný text"),
                ].into_iter().map(move |(label,prefix,suffix,placeholder)| view! {
                    <button class="button secondary" type="button" disabled=move||busy.get()
                        on:mousedown=move |ev|ev.prevent_default()
                        on:click=move |_|insert.run((prefix.into(),suffix.into(),placeholder.into()))>{label}</button>
                }).collect_view()}
            </div>
            <label class="admin-field" for="page-content"><span>"Obsah stránky *"</span>
                <textarea node_ref=textarea id="page-content" required maxlength="100000" rows="16"
                    aria-describedby="markdown-help" prop:value=move||content.get()
                    on:input=move |ev|{content.set(event_target_value(&ev));ctx.dirty.set(true);}></textarea>
            </label>
            <details id="markdown-help" class="markdown-help"><summary>"Nápověda k Markdownu"</summary>
                <p>"Označte text a použijte tlačítko formátování, nebo pište Markdown přímo. Podporované jsou nadpisy, tučný text, kurzíva, seznamy, odkazy, citace, kód, tabulky a přeškrtnutí."</p>
                <pre>{"## Nadpis\n**Tučně** a *kurzíva*\n- Položka seznamu\n[Kontakt](/kontakt)\n> Citace\n`Kód` a ~~přeškrtnutí~~\n\n| Den | Hodiny |\n| --- | --- |\n| Pondělí | 8–12 |"}</pre>
                <p>"Obrázky nahrajte níže. Jejich popis slouží čtečkám obrazovky. HTML se zobrazí jako text."</p>
            </details>
            <ErrorMessage error/>
            <section class="page-image-upload" aria-label="Obrázky stránky">
                <h3>"Obrázky stránky"</h3>
                {if id==0 {view! {
                    <p class="admin-caption">"Obrázky přidáte po prvním uložení stránky. Uložte ji jako koncept, nahrajte obrázky a potom ji zveřejněte."</p>
                }.into_any()} else {view! {
                    <label class="admin-field" for="page-image-alt"><span>"Popis obrázku"</span>
                        <input id="page-image-alt" type="text" maxlength="300" prop:value=move||alt.get()
                            on:input=move|ev|alt.set(event_target_value(&ev))/>
                        <small>"Stručně popište, co obrázek zachycuje. Popis se vloží do textu spolu s obrázkem."</small>
                    </label>
                    <div class="admin-upload">
                        <label for="page-image-file"><strong>"Vybrat obrázek"</strong><span>"PNG, JPEG, WebP nebo GIF. Nejvýše 10 MiB a 8192 × 8192 bodů."</span></label>
                        <input node_ref=input id="page-image-file" type="file" accept="image/png,image/jpeg,image/webp,image/gif" disabled=move||busy.get()
                            on:change=move |_|{
                                error.set(None);
                                let file=input.get().and_then(|el|el.files()).and_then(|files|files.item(0));
                                if file.as_ref().is_some_and(|f|f.size()>10.0*1024.0*1024.0) {
                                    selected.set(false);
                                    error.set(Some(api::Failure{status:413,message:"Obrázek je větší než 10 MiB.".into()}));
                                } else {selected.set(file.is_some());}
                            }/>
                        <button class="button secondary" type="button" disabled=move||busy.get()||!selected.get()||alt.get().trim().is_empty()
                            on:click=move |_|{
                                if !busy.get_untracked() && !alt.get_untracked().trim().is_empty() {
                                    if let Some(file)=input.get().and_then(|el|el.files()).and_then(|files|files.item(0)) {
                                        working.set(true);
                                        upload.dispatch(file);
                                    }
                                }
                            }>{move||if upload.pending().get(){"Nahrávám…"}else{"Nahrát a vložit obrázek"}}</button>
                    </div>
                    <details class="page-image-library"><summary>"Dříve nahrané obrázky"</summary>
                        <p class="admin-caption">"Vyplňte popis a vložte vybraný obrázek do textu. Soubory se uchovávají i pro historii stránky. Veřejně jsou dostupné jen obrázky použité v uloženém, zveřejněném textu."</p>
                        <Suspense fallback=Pending>{move||images.get().map(|result|match result {
                            Err(error)=>view!{<FailureView error/><button class="button secondary" type="button" on:click=move |_|images.refetch()>"Načíst znovu"</button>}.into_any(),
                            Ok(items)=>view!{<ul class="page-image-list">{items.into_iter().map(move |image|view!{
                                <li><img src=format!("/api/v1/page-images/{}",image.id) alt="" loading="lazy"/>
                                    <div><strong>{image.name}</strong><small>{format!("{} × {} · {} kB",image.width,image.height,(image.size_bytes+1023)/1024)}</small></div>
                                    <button class="button secondary" type="button" disabled=move||busy.get()||alt.get().trim().is_empty()
                                        on:click=move |_|{
                                            if let Some(el)=textarea.get(){let end=el.selection_end().ok().flatten().unwrap_or(0);let _=el.set_selection_range(end,end);}
                                            insert.run((image_markdown(&alt.get_untracked(),image.id),String::new(),String::new()));
                                        }>"Vložit do textu"</button>
                                </li>
                            }).collect_view()}</ul>}.into_any(),
                        })}</Suspense>
                    </details>
                }.into_any()}}
            </section>
            <button class="button secondary" type="button" aria-expanded=move||preview.get().to_string() on:click=move |_|preview.update(|v|*v=!*v)>{move||if preview.get(){"Zavřít náhled"}else{"Náhled textu"}}</button>
            <Show when=move||preview.get()><section class="admin-page-preview" aria-label="Náhled textu stránky"><h2>{move||title.get()}</h2><div class="markdown-content" inner_html=move||crate::markdown::render(&content.get())></div></section></Show>
        </div>
    }
}
