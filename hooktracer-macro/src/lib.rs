use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::{quote, format_ident};
use syn::{parse_macro_input, ItemFn, AttributeArgs, NestedMeta, Meta, Lit, FnArg, ReturnType, PatType, Error, Result, Signature};

fn parse_hook_attributes(args: &[NestedMeta]) -> Result<(String, Option<String>)> {
    let mut symbol = None;
    let mut library = None;

    for arg in args {
        if let NestedMeta::Meta(Meta::NameValue(nv)) = arg {
            if nv.path.is_ident("symbol") {
                if let Lit::Str(lit) = &nv.lit {
                    symbol = Some(lit.value());
                } else {
                    return Err(Error::new_spanned(&nv.lit, "Expected a string literal for symbol"));
                }
            } else if nv.path.is_ident("library") {
                if let Lit::Str(lit) = &nv.lit {
                    library = Some(lit.value());
                } else {
                    return Err(Error::new_spanned(&nv.lit, "Expected a string literal for library"));
                }
            } else {
                return Err(Error::new_spanned(&nv.path, "Unknown attribute key, expected 'symbol' or 'library'"));
            }
        } else {
             return Err(Error::new_spanned(arg, "Expected name-value attribute, like symbol = \"...\""));
        }
    }

    match symbol {
        Some(s) => Ok((s, library)),
        None => Err(Error::new(
            Span::call_site(),
            "Missing required attribute 'symbol = \"...\"'",
        )),
    }
}

fn validate_user_fn_signature(sig: &Signature) -> Result<()> {
    if sig.inputs.is_empty() {
        return Err(Error::new_spanned(
            &sig.inputs,
            "Hook logic function must accept at least one argument (the original function pointer)",
        ));
    }

    match sig.inputs.first().unwrap() { // .unwrap() is safe due to the is_empty() check
        FnArg::Typed(PatType { ty, .. }) => {
            if let syn::Type::BareFn(_) = **ty {
                // First argument is a function pointer, this is expected.
            } else {
                return Err(Error::new_spanned(
                    ty,
                    "The first argument of the hook logic function must be a function pointer (e.g., `fn_ptr: unsafe extern \"C\" fn(...) -> ...`)",
                ));
            }
        }
        FnArg::Receiver(_) => {
            return Err(Error::new_spanned(
                sig.inputs.first().unwrap(),
                "Hook logic function cannot have a 'self' receiver as its first argument",
            ));
        }
    }
    // 其他检查可以根据需要添加
    Ok(())
}

#[proc_macro_attribute]
pub fn hook_trace(args: TokenStream, item: TokenStream) -> TokenStream {
    let attr_args = parse_macro_input!(args as AttributeArgs);
    let user_logic_fn = parse_macro_input!(item as ItemFn);

    let (symbol_name, library_name) = match parse_hook_attributes(&attr_args) {
        Ok(val) => val,
        Err(err) => return err.to_compile_error().into(),
    };

    if let Err(err) = validate_user_fn_signature(&user_logic_fn.sig) {
         return err.to_compile_error().into();
    }

    let user_logic_fn_ident = &user_logic_fn.sig.ident; // 用户写的 Rust 函数名
    let user_logic_fn_inputs = &user_logic_fn.sig.inputs; // 用户写的 Rust 函数的参数列表
    let user_logic_fn_output = &user_logic_fn.sig.output; // 用户写的 Rust 函数的返回类型

    let wrapper_fn_inputs = user_logic_fn_inputs.iter().skip(1).collect::<Vec<_>>();
    let wrapper_fn_arg_idents = wrapper_fn_inputs.iter().map(|arg| {
        if let FnArg::Typed(PatType { pat, .. }) = arg {
            pat
        } else {
            unreachable!("Expected typed arguments like 'ident: Type'")
        }
    }).collect::<Vec<_>>();

    // 构造目标 C 函数的标识符（用于 #[no_mangle]）
    let target_fn_ident = format_ident!("{}", symbol_name); // e.g., malloc

    let original_fn_ptr_inputs = wrapper_fn_inputs.iter().map(|arg| {
         if let FnArg::Typed(PatType { ty, .. }) = arg {
             ty // 返回类型
         } else {
             unreachable!()
         }
    }).collect::<Vec<_>>();
    let original_fn_ptr_output = match user_logic_fn_output {
        ReturnType::Default => quote! { -> () },
        ReturnType::Type(_, ty) => quote! { -> #ty },
    };
    let original_fn_ptr_type = quote! { unsafe extern "C" fn(#(#original_fn_ptr_inputs),*) #original_fn_ptr_output };

    let static_original_fn_ident = format_ident!("H_{}_ORIGINAL", symbol_name.to_uppercase());

    let symbol_name_cstr = format!("{}\0", symbol_name);
    let library_handle = match library_name {
        None | Some(_) => quote! { libc::RTLD_NEXT },
    };

    let expanded = quote! {
        #user_logic_fn

        // 生成用于存储原始函数指针的 static OnceLock
        // 使用 std::sync::OnceLock (Rust 1.70+) 或 once_cell::sync::OnceCell
        static #static_original_fn_ident: std::sync::OnceLock<#original_fn_ptr_type> = std::sync::OnceLock::new();

        // 生成 extern "C" 包装函数
        #[no_mangle] // 防止 Rust name mangling，确保 C 能找到它
        pub unsafe extern "C" fn #target_fn_ident(#(#wrapper_fn_inputs),*) #original_fn_ptr_output {
            // 获取原始函数指针，只在第一次调用时执行 dlsym
            let original_fn = #static_original_fn_ident.get_or_init(|| {
                let symbol_name_bytes = #symbol_name_cstr.as_bytes();
                let symbol_name_ptr = symbol_name_bytes.as_ptr() as *const libc::c_char;
                let current_library_handle = #library_handle; // Evaluate the handle

                // 使用 dlsym 查找原始函数地址
                let addr = libc::dlsym(current_library_handle, symbol_name_ptr);

                if addr.is_null() {
                    // dlsym 失败的处理 - 打印错误到 stderr 并中止进程
                    eprintln!(
                        "[hooktracer-macro] FATAL: dlsym failed to find symbol '{}' using handle {:p}. Aborting.",
                        #symbol_name,
                        current_library_handle as *mut libc::c_void
                    );
                    std::process::abort();
                }
                std::mem::transmute::<*mut libc::c_void, #original_fn_ptr_type>(addr)
            });

            #user_logic_fn_ident(*original_fn, #(#wrapper_fn_arg_idents),*)
        }
    };

    expanded.into()
}
