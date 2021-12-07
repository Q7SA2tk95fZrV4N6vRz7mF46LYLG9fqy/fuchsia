// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![allow(dead_code)] // TODO(https://fxbug.dev/77068) remove after toolchain rolls again

use argh::FromArgs;
use heck::SnakeCase;
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs::File,
    io::{BufWriter, Write},
    path::PathBuf,
};
use syn::{
    Token,
    {punctuated::Punctuated, Ident},
};

/// generate a roundtrip test for dynfidl from a file with FIDL's JSON IR.
#[derive(Debug, FromArgs)]
struct Options {
    /// path to the FIDL JSON IR file to use for a round-trip test.
    #[argh(option)]
    input: PathBuf,

    /// output path for the generated Rust test library.
    #[argh(option)]
    output: PathBuf,

    /// path to a rustfmt binary for formatting generated tests
    #[argh(option)]
    rustfmt: PathBuf,

    /// list of types for which to generate round-trip tests
    #[argh(positional)]
    types: Vec<String>,
}

fn main() {
    let Options { input, output, rustfmt, types } = argh::from_env();
    let raw_ir = std::fs::read_to_string(&input).unwrap();
    let FidlIr { struct_declarations } = serde_json::from_str(&raw_ir).unwrap();

    let mut file_tokens = TokenStream::new();
    file_tokens.extend(quote! {
        use dynfidl::{BasicField, Field, Structure, VectorField};
        use fidl::encoding::{
            decode_persistent, Context, Encoder, Persistable, PersistentHeader, PersistentMessage,
            WireFormatVersion, MAGIC_NUMBER_INITIAL,
        };
        use std::{cmp::PartialEq, fmt::Debug};

        #[track_caller]
        fn test_persistent_roundtrip<T: Debug + PartialEq + Persistable>(
            mut domain_value: T,
            structure: Structure,
        ) {
            let context = Context { wire_format_version: WireFormatVersion::V2 };
            let header = PersistentHeader::new_full(&context, MAGIC_NUMBER_INITIAL);
            let mut message = PersistentMessage { header, body: &mut domain_value };
            let mut binding_encoded = Vec::new();
            let mut handles = Vec::new();
            Encoder::encode(&mut binding_encoded, &mut handles, &mut message)
                .expect("must be able to encode domain value using rust bindings");
            assert_eq!(handles.len(), 0, "persistent encoding can't produce any handles");

            let dynamic_encoded = structure.encode_persistent();
            assert_eq!(
                binding_encoded,
                dynamic_encoded,
                "encoded messages from bindings and dynfidl must match",
            );

            let domain_from_dynamic: T = decode_persistent(&dynamic_encoded[..]).unwrap();
            assert_eq!(
                domain_value,
                domain_from_dynamic,
                "domain value from dynamic encoding must match original",
            );
        }

        trait AllIntoBytes {
            fn into_bytes_for_each(self) -> Vec<Vec<u8>>;
        }

        impl AllIntoBytes for Vec<String> {
            fn into_bytes_for_each(self) -> Vec<Vec<u8>> {
                self.into_iter().map(String::into_bytes).collect()
            }
        }
    });

    let mut declarations = struct_declarations
        .into_iter()
        .map(|decl| (decl.name.clone(), decl))
        .filter(|(_, decl)| decl.can_be_encoded_by_dynfidl())
        .collect::<BTreeMap<String, StructDecl>>();

    // emit a test case for each requested type
    for name in types {
        eprintln!("emitting test for {}", name);
        let to_roundtrip = declarations.remove(&name).unwrap();
        to_roundtrip.emit_roundtrip_test(&mut file_tokens);
    }

    // any types still in the declarations from the IR were implicitly skipped by our args
    for (name, _) in declarations {
        let test_name = test_name(&name);
        file_tokens.extend(quote! {
            #[test]
            #[ignore]
            fn #test_name() {}
        })
    }

    {
        // put the output file in a block so it drops/closes before we try formatting
        let mut output_file = BufWriter::new(File::create(&output).unwrap());
        writeln!(
            output_file,
            "
            // Copyright 2021 The Fuchsia Authors. All rights reserved.
            // Use of this source code is governed by a BSD-style license that can be
            // found in the LICENSE file.

            // THIS FILE WAS AUTO-GENERATED BY $OUT_DIR/{}
            #![allow(non_snake_case)] // for easy copy/paste from test output to build config
            {}",
            file!(),
            file_tokens
        )
        .unwrap();
    }

    // format the output file so a human can read it from the out dir
    if !std::process::Command::new(rustfmt).arg(&output).status().unwrap().success() {
        panic!("rustfmt failed!");
    }
}

/// mangle a fidl type name into a test's name. we don't snake/lowercase the ident so that it's
/// easy to copy/paste from terminal output to the list of types to test
fn test_name(name: &str) -> Ident {
    format_ident!("roundtrip_persistent_{}", name.replace("/", "_"))
}

/// returns the rust module path for a given fidl type name
fn domain_value_module_path(name: &str) -> TokenStream {
    let mut parts = name.split("/");
    let library = format_ident!(
        "fidl_{}",
        parts
            .next()
            .expect("must have a library name before a slash")
            .to_snake_case()
            .to_lowercase()
    );
    let ty_name = format_ident!("{}", parts.next().expect("must have a type name after a slash"));
    quote!(#library :: #ty_name)
}

#[derive(Debug, Deserialize)]
struct FidlIr {
    struct_declarations: Vec<StructDecl>,
    // NOTE: the JSON IR has many other fields in the top-level object but we only list those
    // used for generating tests here.
}

#[derive(Debug, Deserialize)]
struct StructDecl {
    name: String,
    resource: bool,
    max_handles: Option<u32>,
    members: Vec<StructMember>,
    type_shape_v2: TypeShape,
    naming_context: Option<Vec<String>>,
    maybe_attributes: Option<Vec<Attribute>>,
    is_request_or_response: Option<bool>,
}

impl StructDecl {
    fn can_be_encoded_by_dynfidl(&self) -> bool {
        // we don't support encoding handles
        if self.resource || self.max_handles.map(|max| max > 0).unwrap_or_default() {
            return false;
        }

        // we don't support structs that are the protocol request/response type (yet?)
        if self.is_request_or_response.unwrap_or_default() {
            return false;
        }

        // check for attributes -- we don't know how to handle those at all
        if self.maybe_attributes.as_ref().map(|attrs| !attrs.is_empty()).unwrap_or_default() {
            return false;
        }

        // check each field for types we don't know how to layout
        for member in &self.members {
            if !member.can_be_encoded_by_dynfidl() {
                return false;
            }
        }

        true
    }

    fn emit_roundtrip_test(&self, tokens: &mut TokenStream) {
        let test_name = test_name(&self.name);
        let domain_value_path = domain_value_module_path(&self.name);

        let mut fidl_domain_fields = Punctuated::<_, Token![,]>::new();
        let mut field_inits = TokenStream::new();
        let mut dynfidl_fields = TokenStream::new();

        for member in &self.members {
            member.initializer(&mut field_inits);
            member.dynfidl_field(&mut dynfidl_fields);
            fidl_domain_fields.push(member.domain_field());
        }

        let fidl_domain_value = quote! { #domain_value_path { #fidl_domain_fields } };
        let dynfidl_structure = quote!(Structure::default() #dynfidl_fields);

        tokens.extend(quote! {
            #[test]
            fn #test_name() {
                #field_inits
                let domain_value = #fidl_domain_value;
                let dynfidl_structure = #dynfidl_structure;
                test_persistent_roundtrip(domain_value, dynfidl_structure);
            }
        })
    }
}

#[derive(Debug, Deserialize)]
struct StructMember {
    name: String,
    r#type: MemberType,
    field_shape_v2: FieldShape,
    maybe_attributes: Option<Vec<Attribute>>,
}

impl StructMember {
    fn can_be_encoded_by_dynfidl(&self) -> bool {
        if self.maybe_attributes.as_ref().map(|a| a.is_empty()).unwrap_or_default() {
            // we don't support attributes at all right now
            return false;
        }

        self.r#type.can_be_encoded_by_dynfidl()
    }

    fn field_name(&self) -> Ident {
        format_ident!("{}", self.name)
    }

    fn initializer(&self, tokens: &mut TokenStream) {
        let field_name = self.field_name();
        let field_init = self.r#type.initializer();
        tokens.extend(quote! { let #field_name = #field_init; });
    }

    fn domain_field(&self) -> TokenStream {
        let field_name = self.field_name();
        quote! { #field_name: #field_name.clone() }
    }

    fn dynfidl_field(&self, tokens: &mut TokenStream) {
        let field_variant = self.r#type.field_variant();
        let inner_variant = self.r#type.inner_variant();
        let field_converter = self.r#type.dynfidl_field_converter();
        let field_name = self.field_name();
        tokens.extend(quote! {
            .field(Field::#field_variant(#inner_variant( #field_name #field_converter)))
        });
    }
}

#[derive(Debug, Deserialize)]
struct Attribute {
    name: String,
    arguments: Vec<AttributeArg>,
}

#[derive(Debug, Deserialize)]
struct AttributeArg {
    name: String,
    r#type: Value,
    value: Value,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind")]
enum MemberType {
    #[serde(rename = "array")]
    Array { element_count: u32, element_type: Box<MemberType>, type_shape_v2: TypeShape },

    #[serde(rename = "vector")]
    Vector {
        element_type: Box<MemberType>,
        nullable: bool,
        type_shape_v2: TypeShape,
        maybe_element_count: Option<u32>,
    },

    #[serde(rename = "string")]
    String { nullable: bool, type_shape_v2: TypeShape, maybe_element_count: Option<u32> },

    #[serde(rename = "handle")]
    Handle { nullable: bool, obj_type: u32, rights: u32, subtype: Value, type_shape_v2: TypeShape },

    #[serde(rename = "request")]
    Request { subtype: String, nullable: bool, type_shape_v2: TypeShape },

    #[serde(rename = "primitive")]
    Basic { subtype: BasicSubtype, nullable: Option<bool>, type_shape_v2: TypeShape },

    #[serde(rename = "identifier")]
    Identifier { identifier: String, nullable: bool, type_shape_v2: TypeShape },
}

impl MemberType {
    fn can_be_encoded_by_dynfidl(&self) -> bool {
        match self {
            Self::Array { .. }
            | Self::Handle { .. }
            | Self::Request { .. }
            | Self::Identifier { .. } => false,
            Self::Vector { element_type, nullable, .. } => {
                // we don't support nullable values yet
                if *nullable {
                    return false;
                }
                element_type.can_be_encoded_by_dynfidl()
            }
            Self::String { nullable, .. } => !nullable,
            Self::Basic { subtype, nullable, .. } => {
                // we don't support nullable basics yet
                if nullable.unwrap_or_default() {
                    return false;
                }

                subtype.can_be_encoded_by_dynfidl()
            }
        }
    }

    fn initializer(&self) -> TokenStream {
        match self {
            Self::Array { .. }
            | Self::Handle { .. }
            | Self::Request { .. }
            | Self::Identifier { .. } => {
                unimplemented!("only basic values and vectors are currently supported")
            }
            Self::String { .. } => quote!(String::from("hello, world!")),
            Self::Vector { element_type, .. } => {
                let mut elements = TokenStream::default();
                for _ in 0..5 {
                    let init = element_type.initializer();
                    elements.extend(quote!(_vec_.push(#init);));
                }
                quote! {{
                    let mut _vec_ = vec![];
                    #elements
                    _vec_
                }}
            }
            Self::Basic { subtype, .. } => subtype.initializer(),
        }
    }

    fn field_variant(&self) -> TokenStream {
        match self {
            Self::Array { .. }
            | Self::Handle { .. }
            | Self::Request { .. }
            | Self::Identifier { .. } => {
                unimplemented!("only basic values and vectors are currently supported")
            }
            Self::Basic { .. } => quote!(Basic),
            Self::String { .. } | Self::Vector { .. } => quote!(Vector),
        }
    }

    fn inner_variant(&self) -> TokenStream {
        match self {
            Self::Array { .. }
            | Self::Handle { .. }
            | Self::Request { .. }
            | Self::Identifier { .. } => {
                unimplemented!("only basic values and vectors are currently supported")
            }
            Self::Vector { element_type, .. } => match &**element_type {
                Self::Array { .. }
                | Self::Handle { .. }
                | Self::Request { .. }
                | Self::Identifier { .. } => unimplemented!("not supported"),
                Self::String { .. } => quote!(VectorField::UInt8VectorVector),
                Self::Vector { element_type, .. } => match &**element_type {
                    Self::Basic { subtype, .. } => match subtype {
                        BasicSubtype::UInt8 => quote!(VectorField::UInt8VectorVector),
                        _ => todo!("nested vectors of non-bytes are not yet supported"),
                    },
                    _ => todo!("nested vectors of non-bytes are not yet supported"),
                },
                Self::Basic { subtype, .. } => match &*subtype {
                    BasicSubtype::UInt8 => quote!(VectorField::UInt8Vector),
                    BasicSubtype::Bool => quote!(VectorField::BoolVector),
                    BasicSubtype::UInt16 => quote!(VectorField::UInt16Vector),
                    BasicSubtype::UInt32 => quote!(VectorField::UInt32Vector),
                    BasicSubtype::UInt64 => quote!(VectorField::UInt64Vector),
                    BasicSubtype::Int8 => quote!(VectorField::Int8Vector),
                    BasicSubtype::Int16 => quote!(VectorField::Int16Vector),
                    BasicSubtype::Int32 => quote!(VectorField::Int32Vector),
                    BasicSubtype::Int64 => quote!(VectorField::Int64Vector),
                    BasicSubtype::Float32 | BasicSubtype::Float64 => {
                        unimplemented!("floats not supported")
                    }
                },
            },
            Self::String { .. } => quote!(VectorField::UInt8Vector),
            Self::Basic { subtype, .. } => subtype.inner_variant(),
        }
    }

    fn dynfidl_field_converter(&self) -> TokenStream {
        match self {
            Self::String { .. } => quote!(.into_bytes()),
            Self::Vector { element_type, .. } => match &**element_type {
                Self::String { .. } => quote!(.into_bytes_for_each()),
                _ => quote!(),
            },
            _ => quote!(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
enum BasicSubtype {
    Bool,
    UInt8,
    UInt16,
    UInt32,
    UInt64,
    Int8,
    Int16,
    Int32,
    Int64,
    Float32,
    Float64,
}

impl BasicSubtype {
    fn can_be_encoded_by_dynfidl(&self) -> bool {
        match self {
            Self::Float32 | Self::Float64 => false,
            _ => true,
        }
    }

    fn initializer(&self) -> TokenStream {
        match self {
            Self::Bool => quote!(true),
            Self::UInt8 => quote!(2u8),
            Self::UInt16 => quote!(3u16),
            Self::UInt32 => quote!(4u32),
            Self::UInt64 => quote!(5u64),
            Self::Int8 => quote!(6i8),
            Self::Int16 => quote!(7i16),
            Self::Int32 => quote!(8i32),
            Self::Int64 => quote!(9i64),
            _ => unimplemented!(),
        }
    }

    fn inner_variant(&self) -> TokenStream {
        match self {
            Self::Bool => quote!(BasicField::Bool),
            Self::UInt8 => quote!(BasicField::UInt8),
            Self::UInt16 => quote!(BasicField::UInt16),
            Self::UInt32 => quote!(BasicField::UInt32),
            Self::UInt64 => quote!(BasicField::UInt64),
            Self::Int8 => quote!(BasicField::Int8),
            Self::Int16 => quote!(BasicField::Int16),
            Self::Int32 => quote!(BasicField::Int32),
            Self::Int64 => quote!(BasicField::Int64),
            _ => unimplemented!(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct TypeShape {
    alignment: u32,
    depth: u32,
    has_flexible_envelope: bool,
    has_padding: bool,
    inline_size: u32,
    max_handles: u32,
    max_out_of_line: u32,
}

#[derive(Debug, Deserialize)]
struct FieldShape {
    offset: u32,
    padding: u32,
}
