//! Ruby outline unit tests, kept out of `outline.rs` (mega-file >800 LOC) per
//! the split-on-touch guardrail. Loaded via `#[cfg(test)] #[path]` from
//! `outline.rs`; production logic stays in `outline.rs` under ~50 changed LOC.

use super::*;

#[test]
fn ruby_module_class_and_methods_outline_structurally() {
    let code = r"
module Billing
  class Invoice < Record
    # Whether the invoice is paid.
    def paid?; end

    def self.find(id); end

    def []=(k, v); end

    def +(other) = other

    class << self
      def build = new
    end
  end
end
";

    let entries = get_outline_entries(code, Lang::Ruby);
    assert_eq!(
        entries.len(),
        1,
        "expected only Billing module: {entries:?}"
    );
    let billing = &entries[0];
    assert_eq!(billing.name, "Billing");
    assert_eq!(billing.kind, OutlineKind::Module);
    assert_eq!(billing.start_line, 2);
    assert_eq!(billing.end_line, 17);

    let invoice = billing
        .children
        .iter()
        .find(|e| e.name == "Invoice")
        .expect("class Invoice under Billing");
    assert_eq!(invoice.kind, OutlineKind::Class);
    assert_eq!(invoice.start_line, 3);
    assert_eq!(invoice.end_line, 16);
    // Inheritance must not leak into the name.
    assert_eq!(invoice.name, "Invoice");

    let names: Vec<&str> = invoice.children.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["paid?", "find", "[]=", "+", "build"],
        "instance + singleton + endless + class << self methods under Invoice"
    );

    let paid = invoice
        .children
        .iter()
        .find(|e| e.name == "paid?")
        .expect("paid? method");
    assert_eq!(paid.kind, OutlineKind::Function);
    assert_eq!(paid.start_line, 5);
    assert_eq!(paid.end_line, 5);
    // tree-sitter-ruby 0.23.1 emits `#` comments as children of the class
    // node (between name and body), not as siblings of methods inside
    // body_statement, so prev-sibling doc extraction does not attach Ruby
    // doc comments. Kept as documented parser evidence; no Ruby doc logic.
    assert!(paid.doc.is_none());

    // No fabricated singleton-class symbol.
    assert!(!names.contains(&"<anonymous>"));
}
