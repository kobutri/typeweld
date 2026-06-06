//! Shared test fixtures for integration tests.

use typeweld_ir::{
    ApiContract, Endpoint, ErrorDef, ErrorRef, ErrorVariant, Field, HttpMethod, HttpStatus,
    Optionality, RequestBodyTransport, RequestShape, ResponseShape, RoutePattern, SourceRange,
    StructShape, SymbolId, Transport, TypeDef, TypeRef, TypeShape,
};

pub const BASIC_PACKAGE_NAME: &str = "@workspace/server-api";

#[must_use]
pub fn basic_contract() -> ApiContract {
    let string = primitive_ref("String");
    let i64_ref = primitive_ref("i64");
    let user_ref = type_ref("fixture:User", "User");
    let create_user_ref = type_ref("fixture:CreateUserRequest", "CreateUserRequest");
    let event_ref = type_ref("fixture:UserEvent", "UserEvent");
    let error_ref = ErrorRef {
        id: SymbolId::new("fixture:GetUserError"),
        name: "GetUserError".to_owned(),
    };

    ApiContract {
        package_name: BASIC_PACKAGE_NAME.to_owned(),
        endpoints: vec![
            Endpoint {
                id: SymbolId::new("fixture:endpoint:getUser"),
                rust_path: rust_path(["fixture", "users", "get_user"]),
                rust_name: "get_user".to_owned(),
                ts_path: ts_path(["users", "getUser"]),
                route: RoutePattern("/users/{id}".to_owned()),
                method: HttpMethod::Get,
                transport: Transport::UnaryHttp,
                request: RequestShape {
                    path_params: vec![field("fixture:field:getUser:id", "id", i64_ref.clone())],
                    query_params: Vec::new(),
                    body: None,
                    body_transport: RequestBodyTransport::Json,
                },
                response: ResponseShape::Json(user_ref.clone()),
                errors: vec![error_ref.clone()],
                source: range("crates/server/src/users.rs", 18, 1, 22, 2),
                router_mounts: Vec::new(),
                allow_unused: false,
            },
            Endpoint {
                id: SymbolId::new("fixture:endpoint:createUser"),
                rust_path: rust_path(["fixture", "users", "create_user"]),
                rust_name: "create_user".to_owned(),
                ts_path: ts_path(["users", "createUser"]),
                route: RoutePattern("/users".to_owned()),
                method: HttpMethod::Post,
                transport: Transport::UnaryHttp,
                request: RequestShape {
                    path_params: Vec::new(),
                    query_params: Vec::new(),
                    body: Some(create_user_ref.clone()),
                    body_transport: RequestBodyTransport::Json,
                },
                response: ResponseShape::Json(user_ref.clone()),
                errors: vec![error_ref.clone()],
                source: range("crates/server/src/users.rs", 24, 1, 28, 2),
                router_mounts: Vec::new(),
                allow_unused: false,
            },
            Endpoint {
                id: SymbolId::new("fixture:endpoint:watchUsers"),
                rust_path: rust_path(["fixture", "events", "watch_users"]),
                rust_name: "watch_users".to_owned(),
                ts_path: ts_path(["events", "watchUsers"]),
                route: RoutePattern("/users/events".to_owned()),
                method: HttpMethod::Get,
                transport: Transport::ServerSentEvents,
                request: RequestShape::default(),
                response: ResponseShape::Stream(event_ref.clone()),
                errors: vec![error_ref.clone()],
                source: range("crates/server/src/events.rs", 10, 1, 14, 2),
                router_mounts: Vec::new(),
                allow_unused: false,
            },
        ],
        types: vec![
            TypeDef {
                id: user_ref.id.clone(),
                rust_path: rust_path(["fixture", "User"]),
                rust_name: "User".to_owned(),
                ts_name: "User".to_owned(),
                shape: TypeShape::Struct(StructShape {
                    fields: vec![
                        field("fixture:field:User:id", "id", i64_ref.clone()),
                        field(
                            "fixture:field:User:displayName",
                            "display_name",
                            string.clone(),
                        ),
                    ],
                }),
                source: range("crates/server/src/types.rs", 5, 1, 9, 2),
            },
            TypeDef {
                id: create_user_ref.id.clone(),
                rust_path: rust_path(["fixture", "CreateUserRequest"]),
                rust_name: "CreateUserRequest".to_owned(),
                ts_name: "CreateUserRequest".to_owned(),
                shape: TypeShape::Struct(StructShape {
                    fields: vec![field(
                        "fixture:field:CreateUserRequest:displayName",
                        "display_name",
                        string.clone(),
                    )],
                }),
                source: range("crates/server/src/types.rs", 11, 1, 14, 2),
            },
            TypeDef {
                id: event_ref.id.clone(),
                rust_path: rust_path(["fixture", "UserEvent"]),
                rust_name: "UserEvent".to_owned(),
                ts_name: "UserEvent".to_owned(),
                shape: TypeShape::Struct(StructShape {
                    fields: vec![
                        field("fixture:field:UserEvent:id", "id", i64_ref),
                        field("fixture:field:UserEvent:kind", "kind", string),
                    ],
                }),
                source: range("crates/server/src/events.rs", 4, 1, 8, 2),
            },
        ],
        errors: vec![ErrorDef {
            id: error_ref.id.clone(),
            rust_path: rust_path(["fixture", "GetUserError"]),
            rust_name: "GetUserError".to_owned(),
            ts_name: "GetUserError".to_owned(),
            variants: vec![
                ErrorVariant {
                    id: SymbolId::new("fixture:error:GetUserError:UserNotFound"),
                    rust_name: "UserNotFound".to_owned(),
                    tag: "UserNotFound".to_owned(),
                    status: HttpStatus(404),
                    fields: vec![field(
                        "fixture:field:GetUserError:UserNotFound:id",
                        "id",
                        primitive_ref("i64"),
                    )],
                    source: range("crates/server/src/errors.rs", 7, 3, 8, 4),
                },
                ErrorVariant {
                    id: SymbolId::new("fixture:error:GetUserError:PermissionDenied"),
                    rust_name: "PermissionDenied".to_owned(),
                    tag: "PermissionDenied".to_owned(),
                    status: HttpStatus(403),
                    fields: Vec::new(),
                    source: range("crates/server/src/errors.rs", 10, 3, 10, 20),
                },
            ],
            source: range("crates/server/src/errors.rs", 4, 1, 12, 2),
        }],
    }
}

fn type_ref(id: &str, name: &str) -> TypeRef {
    TypeRef {
        id: SymbolId::new(id),
        name: name.to_owned(),
    }
}

fn primitive_ref(name: &str) -> TypeRef {
    type_ref(&format!("primitive:{name}"), name)
}

fn field(id: &str, rust_name: &str, type_ref: TypeRef) -> Field {
    let ts_name = match rust_name {
        "display_name" => "displayName",
        other => other,
    };
    Field {
        id: SymbolId::new(id),
        rust_name: rust_name.to_owned(),
        wire_name: ts_name.to_owned(),
        ts_name: ts_name.to_owned(),
        type_ref,
        optionality: Optionality::Required,
        source: range("crates/server/src/types.rs", 1, 1, 1, 1),
    }
}

fn rust_path<const N: usize>(parts: [&str; N]) -> Vec<String> {
    parts.into_iter().map(str::to_owned).collect()
}

fn ts_path<const N: usize>(parts: [&str; N]) -> Vec<String> {
    rust_path(parts)
}

fn range(
    file: &str,
    start_line: u32,
    start_column: u32,
    end_line: u32,
    end_column: u32,
) -> SourceRange {
    SourceRange {
        file: file.to_owned(),
        start_line,
        start_column,
        end_line,
        end_column,
        full_range: None,
    }
}
