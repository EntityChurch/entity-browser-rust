//! Key Manager DOM renderer — pure consumer of
//! [`KeyManagerOutput`](crate::views::key_manager::output::KeyManagerOutput).

use crate::dom::util;
use crate::views::key_manager::output::KeyManagerOutput;

use web_sys::Element;

pub fn render(container: &Element, output: &KeyManagerOutput) {
    container.set_inner_html(&build_html(output));
}

fn build_html(output: &KeyManagerOutput) -> String {
    let mut html = String::from(
        "<div style='padding:12px'>\
         <h2 style='margin:0 0 4px'>Key Manager</h2>\
         <p style='color:var(--text-dim, #888);margin:0 0 12px'>Hosted-peer public identities (Ed25519)</p>\
         <div style='overflow-x:auto'>\
         <table style='width:100%;border-collapse:collapse;font-size:13px'>\
         <tr style='border-bottom:1px solid var(--border, #333)'>\
         <th style='text-align:left;padding:4px'>Label</th>\
         <th style='text-align:left;padding:4px'>Peer ID</th>\
         <th style='text-align:left;padding:4px'>Role</th></tr>",
    );
    for key in &output.keys {
        html.push_str(&format!(
            "<tr style='border-bottom:1px solid #222'>\
             <td style='padding:4px'>{}</td>\
             <td style='padding:4px;font-family:monospace'>{}</td>\
             <td style='padding:4px'>{}</td></tr>",
            util::escape_html(&key.label),
            util::escape_html(&key.peer_id),
            util::escape_html(&key.role)
        ));
    }
    html.push_str("</table></div></div>");
    html
}
