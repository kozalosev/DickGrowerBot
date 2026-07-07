//! Procedural macros to declare, register and increment Prometheus counters right on
//! Teloxide handler functions, without touching the central `metrics` module.

use proc_macro::TokenStream;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::{parse_macro_input, parse_quote, Ident, ItemFn, LitStr, Token};

/// Declares a counter for a command handler and increments it at the beginning of the function.
///
/// The positional argument is the command name: `command_handler("help")` derives the metric name
/// `command_help_usage_total` and the help text `count of /help invocations`. Pass `help = "..."`
/// to override the help text.
///
/// The counter is registered by `metrics::init()` at startup (via a `linkme` distributed slice),
/// so it appears in the `/metrics` output with the value of zero even before the first call.
///
/// ```ignore
/// #[metrics::command_handler("help")]
/// pub async fn help_cmd_handler(...) -> HandlerResult { ... }
/// ```
#[proc_macro_attribute]
pub fn command_handler(attr: TokenStream, item: TokenStream) -> TokenStream {
    expand(attr, item, Kind::Command, None)
}

/// Declares a counter for a handler that answers an inline query, labeled `state = "query"`.
///
/// The positional argument is the metric token: `inline_handler("inline")` derives the metric name
/// `inline_usage_total` and increments its `state="query"` series. Pass `help = "..."` to override
/// the derived help text. Several handlers may share one metric by passing the same token.
///
/// ```ignore
/// #[metrics::inline_handler("inline")]
/// pub async fn inline_handler(...) -> HandlerResult { ... }
/// ```
#[proc_macro_attribute]
pub fn inline_handler(attr: TokenStream, item: TokenStream) -> TokenStream {
    expand(attr, item, Kind::Name, Some(State::Query))
}

/// Declares a counter for a handler invoked once an inline result is chosen, labeled
/// `state = "chosen"`. Mirrors [`macro@inline_handler`]; usually shares the same metric token.
///
/// ```ignore
/// #[metrics::inline_chosen_handler("inline")]
/// pub async fn inline_chosen_handler(...) -> HandlerResult { ... }
/// ```
#[proc_macro_attribute]
pub fn inline_chosen_handler(attr: TokenStream, item: TokenStream) -> TokenStream {
    expand(attr, item, Kind::Name, Some(State::Chosen))
}

/// Whether the positional token names a bot command (prefixed with `command_` in the metric name)
/// or is used verbatim as the metric's base name.
enum Kind {
    Command,
    Name,
}

/// The phase of an inline query a handler processes, recorded as the value of the `state` label.
enum State {
    Query,
    Chosen,
}

impl State {
    fn label_value(&self) -> &'static str {
        match self {
            State::Query => "query",
            State::Chosen => "chosen",
        }
    }
}

/// The shared body of the attribute macros. `state`, when set, becomes a label identifying the
/// phase of an inline query the annotated function handles.
fn expand(attr: TokenStream, item: TokenStream, kind: Kind, state: Option<State>) -> TokenStream {
    let args = parse_macro_input!(attr as MetricArgs);
    let mut func = parse_macro_input!(item as ItemFn);

    let counter = CounterSpec::new(args, kind, state);
    let name = counter.name;
    let help = counter.help;
    let (label_names, label_values): (Vec<String>, Vec<String>) = counter.labels.into_iter().unzip();

    let desc_stmt: syn::Stmt = parse_quote! {
        #[linkme::distributed_slice(crate::metrics::COUNTERS)]
        static __METRICS_COUNTER_DESC: crate::metrics::CounterDesc = crate::metrics::CounterDesc {
            name: #name,
            help: #help,
            label_names: &[#(#label_names),*],
            label_values: &[#(#label_values),*],
        };
    };
    let inc_stmt: syn::Stmt = parse_quote! {
        crate::metrics::inc(&__METRICS_COUNTER_DESC);
    };
    func.block.stmts.insert(0, desc_stmt);
    func.block.stmts.insert(1, inc_stmt);

    quote!(#func).into()
}

/// The arguments of an attribute macro: a required positional token and an optional `help` override.
struct MetricArgs {
    token: String,
    help: Option<String>,
}

impl Parse for MetricArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let token: LitStr = input.parse()?;
        let mut help = None;
        while !input.is_empty() {
            input.parse::<Token![,]>()?;
            if input.is_empty() {
                break; // a trailing comma
            }
            let key: Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            let value: LitStr = input.parse()?;
            match key.to_string().as_str() {
                "help" => help = Some(value.value()),
                _ => return Err(syn::Error::new(key.span(),
                    "unexpected argument; only `help` is supported")),
            }
        }
        Ok(Self { token: token.value(), help })
    }
}

struct CounterSpec {
    name: String,
    help: String,
    labels: Vec<(String, String)>,
}

impl CounterSpec {
    fn new(args: MetricArgs, kind: Kind, state: Option<State>) -> Self {
        let token = args.token;
        let (name, default_help) = match kind {
            Kind::Command => (
                format!("command_{token}_usage_total"),
                format!("count of /{token} invocations"),
            ),
            Kind::Name => (
                format!("{token}_usage_total"),
                format!("count of {token} invocations"),
            ),
        };
        let labels = state
            .map(|s| vec![("state".to_string(), s.label_value().to_string())])
            .unwrap_or_default();
        Self {
            name,
            help: args.help.unwrap_or(default_help),
            labels,
        }
    }
}
