use std::collections::BTreeMap;

pub struct ApiSpec {
    pub title: String,
    pub version: String,
    /// Scheme + host of the first server entry, e.g. `https://api.example.com`.
    pub base_url: Option<String>,
    /// Path portion of the first server entry, e.g. `/dmgt-api/v1`.
    pub prefix: String,
    pub operations: Vec<Operation>,
    pub schemas: BTreeMap<String, Schema>,
}

#[derive(Clone, Copy)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
    Patch,
    Head,
    Options,
}

impl HttpMethod {
    pub fn from_str(s: &str) -> Option<Self> {
        Some(match s.to_ascii_lowercase().as_str() {
            "get" => Self::Get,
            "post" => Self::Post,
            "put" => Self::Put,
            "delete" => Self::Delete,
            "patch" => Self::Patch,
            "head" => Self::Head,
            "options" => Self::Options,
            _ => return None,
        })
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Get => "get",
            Self::Post => "post",
            Self::Put => "put",
            Self::Delete => "delete",
            Self::Patch => "patch",
            Self::Head => "head",
            Self::Options => "options",
        }
    }
}

#[derive(Clone)]
pub struct Operation {
    pub method: HttpMethod,
    /// The raw path as written in the spec, placeholders included.
    pub path: String,
    pub operation_id: Option<String>,
    pub summary: Option<String>,
    pub deprecated: bool,
    pub parameters: Vec<Parameter>,
    pub request_body: Option<RequestBody>,
    /// Preferred 2xx response (media type + schema), if any.
    pub success: Option<SuccessResponse>,
    /// 4xx/5xx/default responses carrying a JSON schema, used for error model analysis.
    pub error_schemas: Vec<Schema>,
}

#[derive(Clone)]
pub struct Parameter {
    pub location: Location,
    pub wire_name: String,
    pub required: bool,
    pub schema: Schema,
    pub description: Option<String>,
}

#[derive(Clone, Copy)]
pub enum Location {
    Path,
    Query,
    Header,
    Cookie,
}

#[derive(Clone)]
pub struct RequestBody {
    /// Ordered (media type, schema) entries.
    pub content: Vec<(String, Schema)>,
}

#[derive(Clone)]
pub struct SuccessResponse {
    pub media_type: String,
    pub schema: Schema,
}

#[derive(Clone)]
pub struct TypeExpr {
    pub schema: Schema,
    pub nullable: bool,
}

#[derive(Clone)]
pub enum Schema {
    /// Reference to a named schema registered in `ApiSpec::schemas`.
    Ref(String),
    Object(ObjectSchema),
    Array(Box<TypeExpr>),
    Str(StrSchema),
    Integer(Option<String>),
    Number(Option<String>),
    Boolean,
    Binary,
    Any,
}

#[derive(Clone, Default)]
pub struct ObjectSchema {
    pub fields: Vec<Field>,
}

#[derive(Clone)]
pub struct Field {
    pub wire_name: String,
    pub type_: TypeExpr,
    pub required: bool,
    pub description: Option<String>,
}

#[derive(Clone)]
pub struct StrSchema {
    pub enum_values: Option<Vec<String>>,
}
