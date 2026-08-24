//! `name_hash` for a name, for comparing against a value read out of the client's memory.

fn main() {
    for name in std::env::args().skip(1) {
        let hash = skysaga_core::name_hash(&name);
        println!("{name:20} {hash:#010x}  {hash}");
    }
}
