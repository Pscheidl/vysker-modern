//! Samostatný frontend pro statický hosting.

fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(obecni_web::app::App);
}
