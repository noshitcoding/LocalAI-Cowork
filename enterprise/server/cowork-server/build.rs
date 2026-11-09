// SQLx embeds external migration files; stable Cargo must watch additions too.
fn main() {
    println!("cargo:rerun-if-changed=migrations");
}
