use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::{quote, format_ident};
use syn::{parse_macro_input, ItemFn, AttributeArgs, NestedMeta, Meta, Lit, FnArg, ReturnType, PatType, Error, Result, Signature};

// 辅助函数：从属性参数中解析出 "symbol" 和 "library"
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
            // 如果能在 `args` 为空时提供更好的 Span 就更好了
            Span::call_site(), // 作为备选
            "Missing required attribute 'symbol = \"...\"'",
        )),
    }
}

// 辅助函数：验证用户提供的逻辑函数签名
// 期望第一个参数是原始函数指针，其余参数与目标 C 函数匹配
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
    // 1. 解析宏属性参数 (AttributeArgs -> Vec<NestedMeta>)
    let attr_args = parse_macro_input!(args as AttributeArgs);
    // 2. 解析被注解的项，我们期望它是一个函数 (ItemFn)
    let user_logic_fn = parse_macro_input!(item as ItemFn);

    // 3. 从属性参数中提取目标符号名和库名
    let (symbol_name, library_name) = match parse_hook_attributes(&attr_args) {
        Ok(val) => val,
        Err(err) => return err.to_compile_error().into(),
    };

    // 4. 验证用户提供的逻辑函数签名（基本检查）
    if let Err(err) = validate_user_fn_signature(&user_logic_fn.sig) {
         return err.to_compile_error().into();
    }

    // 5. 提取用户逻辑函数的信息
    let user_logic_fn_ident = &user_logic_fn.sig.ident; // 用户写的 Rust 函数名
    let user_logic_fn_inputs = &user_logic_fn.sig.inputs; // 用户写的 Rust 函数的参数列表
    let user_logic_fn_output = &user_logic_fn.sig.output; // 用户写的 Rust 函数的返回类型

    // 6. 构造生成的 extern "C" 包装函数的签名
    //    它应该与目标 C 函数签名一致，即用户逻辑函数签名 *去掉* 第一个参数（原始函数指针）
    let wrapper_fn_inputs = user_logic_fn_inputs.iter().skip(1).collect::<Vec<_>>();
    // 提取包装函数参数的标识符，用于在函数体中传递
    let wrapper_fn_arg_idents = wrapper_fn_inputs.iter().map(|arg| {
        if let FnArg::Typed(PatType { pat, .. }) = arg {
            pat // 返回标识符模式
        } else {
            // 简化处理：这里假设都是 `ident: Type` 形式，忽略 self 等情况
            // 生产级代码需要更复杂的模式匹配
            unreachable!("Expected typed arguments like 'ident: Type'")
        }
    }).collect::<Vec<_>>();

    // 7. 构造目标 C 函数的标识符（用于 #[no_mangle]）
    let target_fn_ident = format_ident!("{}", symbol_name); // e.g., malloc

    // 8. 构造原始函数指针的类型
    //    需要提取包装函数的参数类型和返回类型来构建 fn(...) -> ...
    let original_fn_ptr_inputs = wrapper_fn_inputs.iter().map(|arg| {
         if let FnArg::Typed(PatType { ty, .. }) = arg {
             ty // 返回类型
         } else {
             unreachable!()
         }
    }).collect::<Vec<_>>();
    let original_fn_ptr_output = match user_logic_fn_output {
        ReturnType::Default => quote! { -> () }, // C void 返回类型通常对应 Rust ()
        ReturnType::Type(_, ty) => quote! { -> #ty },
    };
    let original_fn_ptr_type = quote! { unsafe extern "C" fn(#(#original_fn_ptr_inputs),*) #original_fn_ptr_output };

    // 9. 构造用于存储原始函数指针的 static OnceLock 的标识符
    //    使用大写加 H_ 前缀以避免冲突
    let static_original_fn_ident = format_ident!("H_{}_ORIGINAL", symbol_name.to_uppercase());

    // 10. 准备 dlsym 需要的 C 字符串
    let symbol_name_cstr = format!("{}\0", symbol_name); // 手动添加 null 终止符
    let library_handle = match library_name {
        // 如果指定了库名，使用 dlopen 获取句柄（这里简化，直接用RTLD_NEXT替代）
        // 生产代码需要处理 dlopen 失败等情况
        // Some(lib) => quote! { /* dlopen logic */ },
        // RTLD_NEXT 在 dlsym 中查找下一个符号，通常用于拦截
        None | Some(_) => quote! { libc::RTLD_NEXT },
    };

    // 11. 使用 quote! 生成最终代码
    let expanded = quote! {
        // 包含用户编写的原始逻辑函数
        #user_logic_fn

        // 生成用于存储原始函数指针的 static OnceLock
        // 使用 std::sync::OnceLock (Rust 1.70+) 或 once_cell::sync::OnceCell
        // 这里使用 OnceLock 示例
        static #static_original_fn_ident: std::sync::OnceLock<#original_fn_ptr_type> = std::sync::OnceLock::new();

        // 生成 extern "C" 包装函数
        #[no_mangle] // 防止 Rust name mangling，确保 C 能找到它
        pub unsafe extern "C" fn #target_fn_ident(#(#wrapper_fn_inputs),*) #original_fn_ptr_output {
            // 获取原始函数指针，只在第一次调用时执行 dlsym
            let original_fn = #static_original_fn_ident.get_or_init(|| {
                let symbol_name_bytes = #symbol_name_cstr.as_bytes();
                // dlsym 需要 *const i8
                let symbol_name_ptr = symbol_name_bytes.as_ptr() as *const libc::c_char;

                // 使用 dlsym 查找原始函数地址
                let addr = libc::dlsym(#library_handle, symbol_name_ptr);

                if addr.is_null() {
                    // dlsym 失败的处理 - 打印错误到 stderr 并中止进程
                    // 这段代码将在生成的函数中运行
                    eprintln!("hooktracer: dlsym failed to find symbol '{}'. Aborting.", #symbol_name);
                    std::process::abort();
                }

                // 将地址转换为正确的函数指针类型
                // 这是 unsafe 的核心！必须确保类型签名匹配
                std::mem::transmute::<*mut libc::c_void, #original_fn_ptr_type>(addr)
            });

            // 调用用户提供的 Rust 逻辑函数
            // 将获取到的原始函数指针作为第一个参数传递
            #user_logic_fn_ident(*original_fn, #(#wrapper_fn_arg_idents),*)
        }
    };

    // 返回生成的 TokenStream
    expanded.into()
}
