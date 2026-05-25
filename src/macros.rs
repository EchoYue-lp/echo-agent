//! Declarative convenience macros
//!
//! Provides `agent!`, `messages!`, `tool_params!`, `chat_request!` and other
//! macros for quickly building common objects, lowering the framework's
//! entry barrier.

/// Quickly create an Agent (declarative syntax, replaces builder chaining).
///
/// # Examples
///
/// ```rust,no_run
/// use echo_agent::prelude::*;
///
/// # fn example() -> echo_agent::error::Result<()> {
/// let mut agent = echo_agent::agent! {
///     model: "qwen3-max",
///     system_prompt: "You are a helpful assistant",
/// }?;
///
/// // With tools (use any type implementing Tool, e.g. echo_tools builtins)
/// let mut agent = echo_agent::agent! {
///     model: "qwen3-max",
///     system_prompt: "You are an assistant",
///     // tools: [my_calculator_tool, my_weather_tool],
///     max_iterations: 15,
/// }?;
/// # Ok(())
/// # }
/// ```
#[macro_export]
macro_rules! agent {
    // terminal
    (@build $b:expr $(,)?) => { $b.build() };

    (@build $b:expr, model: $v:expr, $($rest:tt)*) => {
        $crate::agent!(@build $b.model($v), $($rest)*)
    };
    (@build $b:expr, system_prompt: $v:expr, $($rest:tt)*) => {
        $crate::agent!(@build $b.system_prompt($v), $($rest)*)
    };
    (@build $b:expr, name: $v:expr, $($rest:tt)*) => {
        $crate::agent!(@build $b.name($v), $($rest)*)
    };
    (@build $b:expr, max_iterations: $v:expr, $($rest:tt)*) => {
        $crate::agent!(@build $b.max_iterations($v), $($rest)*)
    };
    (@build $b:expr, token_limit: $v:expr, $($rest:tt)*) => {
        $crate::agent!(@build $b.token_limit($v), $($rest)*)
    };
    (@build $b:expr, llm_config: $v:expr, $($rest:tt)*) => {
        $crate::agent!(@build $b.llm_config($v), $($rest)*)
    };
    (@build $b:expr, session_id: $v:expr, $($rest:tt)*) => {
        $crate::agent!(@build $b.session_id($v), $($rest)*)
    };
    (@build $b:expr, conversation_id: $v:expr, $($rest:tt)*) => {
        $crate::agent!(@build $b.conversation_id($v), $($rest)*)
    };
    (@build $b:expr, enable_memory: $v:expr, $($rest:tt)*) => {
        $crate::agent!(@build { if $v { $b.enable_memory() } else { $b } }, $($rest)*)
    };
    (@build $b:expr, enable_cot: $v:expr, $($rest:tt)*) => {
        $crate::agent!(@build { if $v { $b.enable_cot() } else { $b.disable_cot() } }, $($rest)*)
    };
    (@build $b:expr, permission_policy: $v:expr, $($rest:tt)*) => {
        $crate::agent!(@build $b.permission_policy(::std::sync::Arc::new($v)), $($rest)*)
    };
    (@build $b:expr, audit_logger: $v:expr, $($rest:tt)*) => {
        $crate::agent!(@build $b.audit_logger(::std::sync::Arc::new($v)), $($rest)*)
    };
    (@build $b:expr, tools: [$($t:expr),* $(,)?], $($rest:tt)*) => {
        $crate::agent!(@build {
            let mut __b = $b.enable_tools();
            $( __b = __b.tool(Box::new($t)); )*
            __b
        }, $($rest)*)
    };
    (@build $b:expr, callbacks: [$($c:expr),* $(,)?], $($rest:tt)*) => {
        $crate::agent!(@build {
            let mut __b = $b;
            $( __b = __b.callback(::std::sync::Arc::new($c)); )*
            __b
        }, $($rest)*)
    };
    (@build $b:expr, guards: [$($g:expr),* $(,)?], $($rest:tt)*) => {
        $crate::agent!(@build {
            let mut __b = $b;
            $( __b = __b.guard(::std::sync::Arc::new($g)); )*
            __b
        }, $($rest)*)
    };

    // entry point
    ( $($body:tt)* ) => {
        $crate::agent!(@build $crate::agent::ReactAgentBuilder::new(), $($body)*)
    };
}

/// Quickly build a message list.
///
/// # Examples
///
/// ```rust
/// use echo_agent::messages;
/// use echo_agent::llm::types::Message;
/// use echo_agent::llm::types::Role;
///
/// let msgs = messages![
///     system("You are an assistant"),
///     user("Hello"),
///     assistant("Hello! How can I help you?"),
///     user("1+1=?"),
/// ];
///
/// assert_eq!(msgs.len(), 4);
/// assert_eq!(msgs[0].role, Role::System);
/// ```
#[macro_export]
macro_rules! messages {
    ( $( $role:ident($content:expr) ),* $(,)? ) => {
        vec![
            $( $crate::llm::types::Message::$role($content.to_string()) ),*
        ]
    };
}

/// Quickly build tool parameter JSON Schema.
///
/// # Examples
///
/// ```rust
/// use echo_agent::tool_params;
///
/// let schema = tool_params! {
///     "expression" => (string, required, "Math expression"),
///     "precision"  => (number, "Decimal precision"),
/// };
/// ```
#[macro_export]
macro_rules! tool_params {
    ( $( $name:literal => $spec:tt ),* $(,)? ) => {{
        let mut __properties = ::serde_json::Map::new();
        let mut __required: Vec<&str> = Vec::new();
        $( $crate::__tool_param_field!(__properties, __required, $name, $spec); )*
        ::serde_json::json!({
            "type": "object",
            "properties": ::serde_json::Value::Object(__properties),
            "required": __required,
        })
    }};
}

#[doc(hidden)]
#[macro_export]
macro_rules! __tool_param_field {
    ($props:expr, $req:expr, $name:literal, ($ty:ident, required, $desc:literal)) => {
        let mut __p = ::serde_json::Map::new();
        __p.insert(
            "type".to_string(),
            ::serde_json::Value::String(stringify!($ty).to_string()),
        );
        __p.insert(
            "description".to_string(),
            ::serde_json::Value::String($desc.to_string()),
        );
        $props.insert($name.to_string(), ::serde_json::Value::Object(__p));
        $req.push($name);
    };
    ($props:expr, $req:expr, $name:literal, ($ty:ident, required)) => {
        let mut __p = ::serde_json::Map::new();
        __p.insert(
            "type".to_string(),
            ::serde_json::Value::String(stringify!($ty).to_string()),
        );
        $props.insert($name.to_string(), ::serde_json::Value::Object(__p));
        $req.push($name);
    };
    ($props:expr, $req:expr, $name:literal, ($ty:ident, $desc:literal)) => {
        let mut __p = ::serde_json::Map::new();
        __p.insert(
            "type".to_string(),
            ::serde_json::Value::String(stringify!($ty).to_string()),
        );
        __p.insert(
            "description".to_string(),
            ::serde_json::Value::String($desc.to_string()),
        );
        $props.insert($name.to_string(), ::serde_json::Value::Object(__p));
    };
    ($props:expr, $req:expr, $name:literal, ($ty:ident)) => {
        let mut __p = ::serde_json::Map::new();
        __p.insert(
            "type".to_string(),
            ::serde_json::Value::String(stringify!($ty).to_string()),
        );
        $props.insert($name.to_string(), ::serde_json::Value::Object(__p));
    };
}

/// Quickly build a chat request.
///
/// # Examples
///
/// ```rust
/// use echo_agent::chat_request;
/// use echo_agent::llm::types::Message;
///
/// let req = chat_request!(
///     messages: [system("You are an assistant"), user("Hello")],
///     temperature: 0.7,
///     max_tokens: 2048,
/// );
/// ```
#[macro_export]
macro_rules! chat_request {
    ( messages: [$( $role:ident($content:expr) ),* $(,)?] $(, $key:ident : $val:expr)* $(,)? ) => {{
        #[allow(unused_mut)]
        let mut req = $crate::llm::ChatRequest {
            messages: vec![
                $( $crate::llm::types::Message::$role($content.to_string()) ),*
            ],
            ..Default::default()
        };
        $( $crate::__chat_request_field!(req, $key, $val); )*
        req
    }};
}

#[doc(hidden)]
#[macro_export]
macro_rules! __chat_request_field {
    ($req:expr, temperature, $v:expr) => {
        $req.temperature = Some($v);
    };
    ($req:expr, max_tokens, $v:expr) => {
        $req.max_tokens = Some($v as u32);
    };
    ($req:expr, tool_choice, $v:expr) => {
        $req.tool_choice = Some($v.to_string());
    };
}

#[cfg(test)]
mod tests {
    use crate::llm::types::{Message, Role};

    #[test]
    fn messages_macro_basic() {
        let msgs = messages![
            system("You are an assistant"),
            user("Hello"),
            assistant("Hello! How can I help you?"),
        ];

        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[0].role, Role::System);
        assert_eq!(msgs[0].content.as_text_ref(), Some("You are an assistant"));
        assert_eq!(msgs[1].role, Role::User);
        assert_eq!(msgs[2].role, Role::Assistant);
    }

    #[test]
    fn messages_macro_single() {
        let msgs = messages![user("hello")];
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].role, Role::User);
    }

    #[test]
    fn messages_macro_empty() {
        let msgs: Vec<Message> = messages![];
        assert!(msgs.is_empty());
    }

    #[test]
    fn tool_params_macro_basic() {
        let schema = tool_params! {
            "expression" => (string, required, "Math expression"),
            "precision"  => (number, "Decimal precision"),
        };

        let obj = schema.as_object().unwrap();
        assert_eq!(obj["type"], "object");

        let props = obj["properties"].as_object().unwrap();
        assert!(props.contains_key("expression"));
        assert!(props.contains_key("precision"));

        let expr_prop = props["expression"].as_object().unwrap();
        assert_eq!(expr_prop["type"], "string");
        assert_eq!(expr_prop["description"], "Math expression");

        let required = obj["required"].as_array().unwrap();
        assert_eq!(required.len(), 1);
        assert_eq!(required[0], "expression");
    }

    #[test]
    fn tool_params_macro_all_required() {
        let schema = tool_params! {
            "a" => (number, required, "param a"),
            "b" => (number, required, "param b"),
        };
        let required = schema["required"].as_array().unwrap();
        assert_eq!(required.len(), 2);
    }

    #[test]
    fn tool_params_macro_none_required() {
        let schema = tool_params! {
            "hint" => (string, "optional hint"),
        };
        let required = schema["required"].as_array().unwrap();
        assert!(required.is_empty());
    }

    #[test]
    fn chat_request_macro_basic() {
        let req = chat_request!(
            messages: [system("You are an assistant"), user("Hello")],
            temperature: 0.7,
            max_tokens: 2048,
        );

        assert_eq!(req.messages.len(), 2);
        assert_eq!(req.messages[0].role, Role::System);
        assert_eq!(req.temperature, Some(0.7));
        assert_eq!(req.max_tokens, Some(2048));
    }

    #[test]
    fn chat_request_macro_no_options() {
        let req = chat_request!(
            messages: [user("hello")],
        );

        assert_eq!(req.messages.len(), 1);
        assert_eq!(req.temperature, None);
        assert_eq!(req.max_tokens, None);
    }

    #[test]
    fn agent_macro_basic() {
        let result = agent! {
            model: "test-model",
            system_prompt: "You are an assistant",
        };
        assert!(result.is_ok());
    }

    #[test]
    fn agent_macro_with_tools_and_options() {
        use crate::tools::builtin::answer::FinalAnswerTool;

        let result = agent! {
            model: "test-model",
            system_prompt: "You are a calculation assistant",
            name: "calc",
            tools: [FinalAnswerTool],
            max_iterations: 5,
        };
        assert!(result.is_ok());

        let agent = result.unwrap();
        assert!(agent.tool_names().contains(&String::from("final_answer")));
    }
}
