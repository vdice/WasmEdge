use crate::{types::ValType, wasmedge, Error, Value, WasmEdgeResult, WasmFnIO, HOST_FUNCS};

use core::ffi::c_void;
use std::convert::TryInto;

use rand::Rng;

extern "C" fn wraper_fn(
    key_ptr: *mut c_void,
    _data: *mut c_void,
    _mem_cxt: *mut wasmedge::WasmEdge_MemoryInstanceContext,
    params: *const wasmedge::WasmEdge_Value,
    param_len: u32,
    returns: *mut wasmedge::WasmEdge_Value,
    return_len: u32,
) -> wasmedge::WasmEdge_Result {
    let key = key_ptr as *const usize as usize;
    let mut result: Result<Vec<Value>, u8> = Err(0);

    let mut input: Vec<Value> = {
        let raw_input = unsafe {
            std::slice::from_raw_parts(
                params,
                param_len
                    .try_into()
                    .expect("len of params should not greater than usize"),
            )
        };
        raw_input.iter().map(|r| (*r).into()).collect()
    };
    input.remove(0);

    let return_len = return_len
        .try_into()
        .expect("len of returns should not greater than usize");
    let raw_returns = unsafe { std::slice::from_raw_parts_mut(returns, return_len) };

    HOST_FUNCS.with(|f| {
        let mut host_functions = f.borrow_mut();
        let real_fn = host_functions
            .remove(&key)
            .expect("host function should there");
        result = real_fn(input);
    });

    match result {
        Ok(v) => {
            assert!(v.len() == return_len);
            for (idx, item) in v.into_iter().enumerate() {
                raw_returns[idx] = item.into();
            }
            wasmedge::WasmEdge_Result { Code: 0 }
        }
        Err(c) => wasmedge::WasmEdge_Result { Code: c as u8 },
    }
}

#[derive(Debug)]
pub struct Function {
    pub(crate) ctx: *mut wasmedge::WasmEdge_FunctionInstanceContext,
    pub(crate) registered: bool,
    pub(crate) ty: Option<FuncType>,
}

impl Function {
    /// wasmedge::WasmEdge_FunctionInstanceCreate take C functions
    /// This may not be implement, base on the limiation of passing C functions in Rust lang.
    /// Please refer [`create_bindings`] for building hostfunctions.
    pub fn create<I: WasmFnIO, O: WasmFnIO>(
        _f: Box<dyn std::ops::Fn(Vec<Value>) -> Vec<Value>>,
    ) -> Self {
        unimplemented!()
    }

    // TODO:
    // binding errors and restict the error types
    #[allow(clippy::type_complexity)]
    /// Binding Rust function (HostFunction) to WasmEdgeFunction
    ///
    /// `I` and `O` are traits base on the input parameters and the output parameters of the
    /// `real_fn`. For example, use `I2<i32, i32>` for the `real_fn` with two i32 input parameters,
    /// and use `I1<i32>` for the `real_fn` with one i32 output parameter.
    pub fn create_bindings<I: WasmFnIO, O: WasmFnIO>(
        real_fn: Box<dyn Fn(Vec<Value>) -> Result<Vec<Value>, u8>>,
    ) -> WasmEdgeResult<Self> {
        let mut key = 0usize;
        HOST_FUNCS.with(|f| {
            let mut rng = rand::thread_rng();
            let mut host_functions = f.borrow_mut();
            assert!(
                host_functions.len() <= host_functions.capacity(),
                "Too many host functions"
            );
            key = rng.gen::<usize>();
            while host_functions.contains_key(&key) {
                key = rng.gen::<usize>();
            }
            host_functions.insert(key, real_fn);
        });

        let key_ptr = key as *const usize as *mut c_void;
        let mut ty = FuncType::create(I::parameters(), O::parameters())?;

        let ctx = unsafe {
            wasmedge::WasmEdge_FunctionInstanceCreateBinding(
                ty.ctx,
                Some(wraper_fn),
                key_ptr,
                std::ptr::null_mut(),
                0,
            )
        };

        match ctx.is_null() {
            true => Err(Error::OperationError(String::from(
                "fail to create host function via the create_bindings interface",
            ))),
            false => {
                ty.ctx = std::ptr::null_mut();
                ty.registered = true;

                Ok(Self {
                    ctx,
                    ty: Some(ty),
                    registered: false,
                })
            }
        }
    }

    /// Get the function type of the function instance.
    pub fn get_type(&self) -> WasmEdgeResult<FuncType> {
        let ty = unsafe { wasmedge::WasmEdge_FunctionInstanceGetFunctionType(self.ctx) };
        match ty.is_null() {
            true => Err(Error::OperationError(String::from(
                "fail to get type info of the function",
            ))),
            false => Ok(FuncType {
                ctx: ty as *mut _,
                registered: true,
            }),
        }
    }
}

impl Drop for Function {
    fn drop(&mut self) {
        self.ty = None;
        if !self.registered && !self.ctx.is_null() {
            unsafe { wasmedge::WasmEdge_FunctionInstanceDelete(self.ctx) };
        }
    }
}

#[derive(Debug)]
pub struct FuncType {
    pub(crate) ctx: *mut wasmedge::WasmEdge_FunctionTypeContext,
    pub(crate) registered: bool,
}
impl FuncType {
    pub(crate) fn create(input: Vec<Value>, output: Vec<Value>) -> WasmEdgeResult<Self> {
        let raw_input = {
            let mut head = vec![wasmedge::WasmEdge_ValType_ExternRef];
            let mut tail = input
                .iter()
                .map(|v| wasmedge::WasmEdge_Value::from(*v).Type)
                .collect::<Vec<wasmedge::WasmEdge_ValType>>();
            head.append(&mut tail);
            head
        };
        let raw_output = output
            .iter()
            .map(|v| wasmedge::WasmEdge_Value::from(*v).Type)
            .collect::<Vec<wasmedge::WasmEdge_ValType>>();

        let ctx = unsafe {
            wasmedge::WasmEdge_FunctionTypeCreate(
                raw_input.as_ptr() as *const u32,
                raw_input.len() as u32,
                raw_output.as_ptr() as *const u32,
                raw_output.len() as u32,
            )
        };
        match ctx.is_null() {
            true => Err(Error::OperationError(String::from(
                "fail to create FuncType instance",
            ))),
            false => Ok(Self {
                ctx,
                registered: false,
            }),
        }
    }

    /// Get the parameter types list length
    pub fn params_len(&self) -> usize {
        unsafe { wasmedge::WasmEdge_FunctionTypeGetParametersLength(self.ctx) as usize }
    }

    /// Get the iterator of the parameter types
    pub fn params_type_iter(&self) -> impl Iterator<Item = ValType> {
        let len = self.params_len();
        let mut types = Vec::with_capacity(len);
        unsafe {
            wasmedge::WasmEdge_FunctionTypeGetParameters(self.ctx, types.as_mut_ptr(), len as u32);
            types.set_len(len);
        }

        types.into_iter().map(Into::into)
    }

    /// Get the return types list length
    pub fn returns_len(&self) -> usize {
        unsafe { wasmedge::WasmEdge_FunctionTypeGetReturnsLength(self.ctx) as usize }
    }

    /// Get the iterator of the return types
    pub fn returns_type_iter(&self) -> impl Iterator<Item = ValType> {
        let len = self.returns_len();
        let mut types = Vec::with_capacity(len);
        unsafe {
            wasmedge::WasmEdge_FunctionTypeGetReturns(self.ctx, types.as_mut_ptr(), len as u32);
            types.set_len(len);
        }

        types.into_iter().map(Into::into)
    }
}
impl Drop for FuncType {
    fn drop(&mut self) {
        if !self.registered && !self.ctx.is_null() {
            unsafe { wasmedge::WasmEdge_FunctionTypeDelete(self.ctx) };
        }
    }
}
