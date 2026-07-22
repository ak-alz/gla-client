//! Embeds the real brand-mark icon (assets/app.rc) into
//! growth-layer-agent.exe on Windows. `embed_resource::compile` is a
//! documented no-op on non-Windows targets, so this file needs no
//! platform cfg-gating of its own -- see Cargo.toml's own comment on
//! why `embed-resource` is still an unconditional (not target-gated)
//! build-dependency despite that.
fn main() {
    embed_resource::compile("assets/app.rc", embed_resource::NONE)
        .manifest_optional()
        .unwrap();
}
