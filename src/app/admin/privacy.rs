use super::{AdminContext, api, ui::*};
use crate::privacy::{PrivacyNotice, PrivacyPolicy, Retention};
use leptos::prelude::*;
use leptos_meta::Title;
use serde_json::json;

#[component]
pub fn PrivacySettings() -> impl IntoView {
    let settings = LocalResource::new(move || async {
        api::get::<Option<PrivacyNotice>>("/api/v1/admin/privacy").await
    });
    view! {
        <Title text="Ochrana údajů · Správa Vyskře"/>
        <Heading title="Ochrana údajů a podmínky odběru" description="Upravte správce, zpracovatele, podmínky odběru a znění souhlasu. Změny se zveřejní po uložení."/>
        <Suspense fallback=Pending>{move ||settings.get().map(|result|match result {
            Ok(initial)=>view!{<PrivacyForm initial/>}.into_any(),
            Err(error)=>view!{<FailureView error/><button class="button secondary" type="button" on:click=move |_|settings.refetch()>"Zkusit znovu"</button>}.into_any(),
        })}</Suspense>
    }
}

#[component]
fn PrivacyForm(initial: Option<PrivacyNotice>) -> impl IntoView {
    let ctx = expect_context::<AdminContext>();
    let fingerprint = RwSignal::new(initial.as_ref().map(|n| n.fingerprint.clone()));
    let initial = initial.map(|n| n.policy).unwrap_or_default();
    let version = RwSignal::new(initial.version.clone());
    let approved_on = RwSignal::new(initial.approved_on.clone().unwrap_or_default());
    let controller_name = RwSignal::new(initial.controller_name.clone());
    let controller_address = RwSignal::new(initial.controller_address.clone());
    let controller_email = RwSignal::new(initial.controller_email.clone());
    let dpo_email = RwSignal::new(initial.dpo_email.clone());
    let processors = RwSignal::new(initial.processors.clone());
    let international_transfers = RwSignal::new(initial.international_transfers.clone());
    let consent_text = RwSignal::new(initial.consent_text.clone());
    let newsletter_terms = RwSignal::new(initial.newsletter_terms.clone());
    let rights_text = RwSignal::new(initial.rights_text.clone());
    let consent_evidence_legal_basis = RwSignal::new(initial.consent_evidence_legal_basis.clone());
    let public_records_legal_basis = RwSignal::new(initial.public_records_legal_basis.clone());
    let security_legal_basis = RwSignal::new(initial.security_legal_basis.clone());
    let pending_days = RwSignal::new(initial.retention.pending_days.to_string());
    let withdrawn_days = RwSignal::new(initial.retention.withdrawn_days.to_string());
    let mail_days = RwSignal::new(initial.retention.mail_days.to_string());
    let audit_days = RwSignal::new(initial.retention.audit_days.to_string());
    let notice_internal_days = RwSignal::new(initial.retention.notice_internal_days.to_string());
    let operational_log_days = RwSignal::new(initial.retention.operational_log_days.to_string());
    let backup_days = RwSignal::new(initial.retention.backup_days.to_string());
    let error = RwSignal::new(None);
    let save = Action::new_local(move |_: &()| async move {
        error.set(None);
        let days = |value: RwSignal<String>| value.get_untracked().parse::<u16>().ok();
        let Some(pending_days) = days(pending_days) else {
            error.set(Some(api::Failure {
                status: 400,
                message: "Vyplňte lhůty celým počtem dní.".into(),
            }));
            return;
        };
        let Some(withdrawn_days) = days(withdrawn_days) else {
            error.set(Some(api::Failure {
                status: 400,
                message: "Vyplňte lhůty celým počtem dní.".into(),
            }));
            return;
        };
        let Some(mail_days) = days(mail_days) else {
            error.set(Some(api::Failure {
                status: 400,
                message: "Vyplňte lhůty celým počtem dní.".into(),
            }));
            return;
        };
        let Some(audit_days) = days(audit_days) else {
            error.set(Some(api::Failure {
                status: 400,
                message: "Vyplňte lhůty celým počtem dní.".into(),
            }));
            return;
        };
        let Some(notice_internal_days) = days(notice_internal_days) else {
            error.set(Some(api::Failure {
                status: 400,
                message: "Vyplňte lhůty celým počtem dní.".into(),
            }));
            return;
        };
        let Some(operational_log_days) = days(operational_log_days) else {
            error.set(Some(api::Failure {
                status: 400,
                message: "Vyplňte lhůty celým počtem dní.".into(),
            }));
            return;
        };
        let Some(backup_days) = days(backup_days) else {
            error.set(Some(api::Failure {
                status: 400,
                message: "Vyplňte lhůty celým počtem dní.".into(),
            }));
            return;
        };
        let policy = PrivacyPolicy {
            version: version.get_untracked(),
            approved_on: (!approved_on.get_untracked().is_empty())
                .then(|| approved_on.get_untracked()),
            controller_name: controller_name.get_untracked(),
            controller_address: controller_address.get_untracked(),
            controller_email: controller_email.get_untracked(),
            dpo_email: dpo_email.get_untracked(),
            processors: processors.get_untracked(),
            international_transfers: international_transfers.get_untracked(),
            consent_text: consent_text.get_untracked(),
            newsletter_terms: newsletter_terms.get_untracked(),
            rights_text: rights_text.get_untracked(),
            consent_evidence_legal_basis: consent_evidence_legal_basis.get_untracked(),
            public_records_legal_basis: public_records_legal_basis.get_untracked(),
            security_legal_basis: security_legal_basis.get_untracked(),
            retention: Retention {
                pending_days,
                withdrawn_days,
                mail_days,
                audit_days,
                notice_internal_days,
                operational_log_days,
                backup_days,
            },
        };
        let body = json!({"policy":policy,"fingerprint":fingerprint.get_untracked()});
        match api::save("PUT", "/api/v1/admin/privacy", Some(body)).await {
            Ok(value)=>match serde_json::from_value::<PrivacyNotice>(value) {
                Ok(saved)=>{
                    fingerprint.set(Some(saved.fingerprint));
                    ctx.dirty.set(false);
                    ctx.changed("Informace o ochraně údajů a podmínky odběru byly uloženy.");
                },
                Err(_)=>error.set(Some(api::Failure {status:0,message:"Server vrátil neočekávanou odpověď. Obnovte stránku a ověřte uložené informace.".into()})),
            },
            Err(failure)=>error.set(Some(failure)),
        }
    });
    view! {
        <form on:submit=move |ev|{ev.prevent_default();if !save.pending().get_untracked(){save.dispatch(());}}>
            <fieldset disabled=move ||save.pending().get()>
                <section class="admin-panel admin-form-section">
                    <h2>"Správce a zpracovatelé"</h2>
                    <p class="admin-caption">"Uložené údaje budou veřejné. Starší souhlasy zůstanou uchované v původním znění."</p>
                    <Field id="privacy-version" label="Verze informací" value=version required=true maxlength="10000"/>
                    <Field id="privacy-approved-on" label="Datum schválení" value=approved_on kind="date" maxlength="10000"/>
                    <Field id="privacy-controller-name" label="Správce osobních údajů" value=controller_name required=true maxlength="10000"/>
                    <Field id="privacy-controller-address" label="Adresa správce" value=controller_address maxlength="10000"/>
                    <Field id="privacy-controller-email" label="E-mail správce" value=controller_email kind="email" required=true maxlength="254"/>
                    <Field id="privacy-dpo-email" label="E-mail pověřence" value=dpo_email kind="email" maxlength="254"/>
                    <TextArea id="privacy-processors" label="Zpracovatelé a příjemci údajů" value=processors required=true maxlength="10000"/>
                    <TextArea id="privacy-international-transfers" label="Předávání do zahraničí" value=international_transfers required=true maxlength="10000"/>
                </section><section class="admin-panel admin-form-section">
                    <h2>"Souhlas a podmínky odběru"</h2>
                    <p class="admin-caption">"Text souhlasu se zobrazuje u přihlašovacího formuláře. Podmínky a práva odběratelů se zveřejní na stránce Ochrana údajů. V těchto dvou polích můžete použít Markdown, například odstavce, seznamy a odkazy."</p>
                    <TextArea id="privacy-consent-text" label="Text souhlasu u formuláře" value=consent_text required=true maxlength="10000"/>
                    <TextArea id="privacy-newsletter-terms" label="Podmínky odběru" value=newsletter_terms required=true maxlength="10000"/>
                    <TextArea id="privacy-rights-text" label="Práva odběratelů" value=rights_text required=true maxlength="10000"/>
                    <details><summary>"Náhled podmínek a práv"</summary><div class="prose markdown-content" inner_html=move ||crate::markdown::render(&format!("{}\n\n{}",newsletter_terms.get(),rights_text.get()))></div></details>
                </section><section class="admin-panel admin-form-section"><h2>"Právní důvody"</h2>
                    <TextArea id="privacy-consent-evidence-legal-basis" label="Právní důvod uchování souhlasu" value=consent_evidence_legal_basis required=true maxlength="10000"/>
                    <TextArea id="privacy-public-records-legal-basis" label="Právní důvody zveřejněných dokumentů" value=public_records_legal_basis required=true maxlength="10000"/>
                    <TextArea id="privacy-security-legal-basis" label="Právní důvod provozu a bezpečnosti" value=security_legal_basis required=true maxlength="10000"/>
                </section><section class="admin-panel admin-form-section">
                    <h2>"Doby uchování ve dnech"</h2>
                    <p class="admin-caption">"Změna aplikačních lhůt se použije při dalším automatickém úklidu. Zkrácení může vést k odstranění starších záznamů. Dobu záloh a provozních logů nastavte podle skutečné rotace na serveru, kterou tento formulář nemění. Nula u záloh znamená, že se vlastní zálohy nevytvářejí."</p>
                    <DaysField id="privacy-pending-days" label="Nepotvrzené žádosti" value=pending_days min=1 max=30/>
                    <DaysField id="privacy-withdrawn-days" label="Údaje po odhlášení" value=withdrawn_days min=1 max=3650/>
                    <DaysField id="privacy-mail-days" label="Historie e-mailů" value=mail_days min=1 max=3650/>
                    <DaysField id="privacy-audit-days" label="Audit správy webu" value=audit_days min=1 max=3650/>
                    <DaysField id="privacy-notice-internal-days" label="Interní doklady úřední desky" value=notice_internal_days min=1 max=3650/>
                    <DaysField id="privacy-operational-log-days" label="Provozní logy" value=operational_log_days min=1 max=3650/>
                    <DaysField id="privacy-backup-days" label="Záložní kopie" value=backup_days min=0 max=3650/>
                </section>
                <ErrorMessage error/>
                <div class="admin-actions"><button class="button primary" type="submit" disabled=move ||save.pending().get()>{move ||if save.pending().get(){"Ukládám…"}else{"Uložit informace"}}</button><a class="text-link" href="/ochrana-udaju" target="_blank" rel="noopener">"Otevřít veřejnou stránku"</a></div>
            </fieldset>
        </form>
    }
}

#[component]
fn DaysField(
    id: &'static str,
    label: &'static str,
    value: RwSignal<String>,
    min: u16,
    max: u16,
) -> impl IntoView {
    let ctx = expect_context::<AdminContext>();
    view! {<label class="admin-field" for=id><span>{label}</span><input id=id type="number" required min=min max=max step="1" prop:value=move ||value.get() on:input=move |ev|{value.set(event_target_value(&ev));ctx.dirty.set(true);}/></label>}
}
