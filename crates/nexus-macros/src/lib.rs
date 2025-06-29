// TODO: create derive macro for message message trait
// TODO: create attribute macro for DomainEvent trait called nexus::

use proc_macro::TokenStream;

#[proc_macro_attribute]
pub fn domain_event(args: TokenStream, input: TokenStream) -> TokenStream {
    // should impl nexus::Message
    let _ = args;
    let _ = input;
    unimplemented!()
}
