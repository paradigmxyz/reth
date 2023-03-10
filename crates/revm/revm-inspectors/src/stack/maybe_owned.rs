use revm::{
    interpreter::{CallInputs, CreateInputs, Gas, InstructionResult, Interpreter},
    primitives::{db::Database, Bytes, B160, B256},
    EVMData, Inspector,
};
use std::{
    cell::{Ref, RefCell},
    rc::Rc,
};

/// An [Inspector] that is either owned by an individual [Inspector] or is shared as part of a
/// series of inspectors in a [InspectorStack](crate::stack::InspectorStack).
///
/// Caution: if the [Inspector] is _stacked_ then it _must_ be called first.
#[derive(Debug)]
pub enum MaybeOwnedInspector<INSP> {
    /// Inspector is owned.
    Owned(Rc<RefCell<INSP>>),
    /// Inspector is shared and part of a stack
    Stacked(Rc<RefCell<INSP>>),
}

impl<INSP> MaybeOwnedInspector<INSP> {
    /// Create a new _owned_ instance
    pub fn new_owned(inspector: INSP) -> Self {
        MaybeOwnedInspector::Owned(Rc::new(RefCell::new(inspector)))
    }

    /// Creates a [MaybeOwnedInspector::Stacked] clone of this type.
    pub fn clone_stacked(&self) -> Self {
        match self {
            MaybeOwnedInspector::Owned(gas) | MaybeOwnedInspector::Stacked(gas) => {
                MaybeOwnedInspector::Stacked(Rc::clone(gas))
            }
        }
    }

    /// Returns a reference to the inspector.
    pub fn as_ref(&self) -> Ref<'_, INSP> {
        match self {
            MaybeOwnedInspector::Owned(insp) => insp.borrow(),
            MaybeOwnedInspector::Stacked(insp) => insp.borrow(),
        }
    }
}

impl<INSP: Default> MaybeOwnedInspector<INSP> {
    /// Create a new _owned_ instance
    pub fn owned() -> Self {
        Self::new_owned(Default::default())
    }
}

impl<INSP: Default> Default for MaybeOwnedInspector<INSP> {
    fn default() -> Self {
        Self::owned()
    }
}

impl<INSP> Clone for MaybeOwnedInspector<INSP> {
    fn clone(&self) -> Self {
        self.clone_stacked()
    }
}

impl<INSP, DB> Inspector<DB> for MaybeOwnedInspector<INSP>
where
    DB: Database,
    INSP: Inspector<DB>,
{
    fn initialize_interp(
        &mut self,
        interp: &mut Interpreter,
        data: &mut EVMData<'_, DB>,
        is_static: bool,
    ) -> InstructionResult {
        match self {
            MaybeOwnedInspector::Owned(insp) => {
                return insp.borrow_mut().initialize_interp(interp, data, is_static)
            }
            MaybeOwnedInspector::Stacked(_) => {}
        }

        InstructionResult::Continue
    }

    fn step(
        &mut self,
        interp: &mut Interpreter,
        data: &mut EVMData<'_, DB>,
        is_static: bool,
    ) -> InstructionResult {
        match self {
            MaybeOwnedInspector::Owned(insp) => {
                return insp.borrow_mut().step(interp, data, is_static)
            }
            MaybeOwnedInspector::Stacked(_) => {}
        }

        InstructionResult::Continue
    }

    fn log(
        &mut self,
        evm_data: &mut EVMData<'_, DB>,
        address: &B160,
        topics: &[B256],
        data: &Bytes,
    ) {
        match self {
            MaybeOwnedInspector::Owned(insp) => {
                return insp.borrow_mut().log(evm_data, address, topics, data)
            }
            MaybeOwnedInspector::Stacked(_) => {}
        }
    }

    fn step_end(
        &mut self,
        interp: &mut Interpreter,
        data: &mut EVMData<'_, DB>,
        is_static: bool,
        eval: InstructionResult,
    ) -> InstructionResult {
        match self {
            MaybeOwnedInspector::Owned(insp) => {
                return insp.borrow_mut().step_end(interp, data, is_static, eval)
            }
            MaybeOwnedInspector::Stacked(_) => {}
        }

        InstructionResult::Continue
    }

    fn call(
        &mut self,
        data: &mut EVMData<'_, DB>,
        inputs: &mut CallInputs,
        is_static: bool,
    ) -> (InstructionResult, Gas, Bytes) {
        match self {
            MaybeOwnedInspector::Owned(insp) => {
                return insp.borrow_mut().call(data, inputs, is_static)
            }
            MaybeOwnedInspector::Stacked(_) => {}
        }

        (InstructionResult::Continue, Gas::new(0), Bytes::new())
    }

    fn call_end(
        &mut self,
        data: &mut EVMData<'_, DB>,
        inputs: &CallInputs,
        remaining_gas: Gas,
        ret: InstructionResult,
        out: Bytes,
        is_static: bool,
    ) -> (InstructionResult, Gas, Bytes) {
        match self {
            MaybeOwnedInspector::Owned(insp) => {
                return insp.borrow_mut().call_end(data, inputs, remaining_gas, ret, out, is_static)
            }
            MaybeOwnedInspector::Stacked(_) => {}
        }
        (ret, remaining_gas, out)
    }

    fn create(
        &mut self,
        data: &mut EVMData<'_, DB>,
        inputs: &mut CreateInputs,
    ) -> (InstructionResult, Option<B160>, Gas, Bytes) {
        match self {
            MaybeOwnedInspector::Owned(insp) => return insp.borrow_mut().create(data, inputs),
            MaybeOwnedInspector::Stacked(_) => {}
        }

        (InstructionResult::Continue, None, Gas::new(0), Bytes::default())
    }

    fn create_end(
        &mut self,
        data: &mut EVMData<'_, DB>,
        inputs: &CreateInputs,
        ret: InstructionResult,
        address: Option<B160>,
        remaining_gas: Gas,
        out: Bytes,
    ) -> (InstructionResult, Option<B160>, Gas, Bytes) {
        match self {
            MaybeOwnedInspector::Owned(insp) => {
                return insp.borrow_mut().create_end(data, inputs, ret, address, remaining_gas, out)
            }
            MaybeOwnedInspector::Stacked(_) => {}
        }

        (ret, address, remaining_gas, out)
    }

    fn selfdestruct(&mut self, contract: B160, target: B160) {
        match self {
            MaybeOwnedInspector::Owned(insp) => {
                return insp.borrow_mut().selfdestruct(contract, target)
            }
            MaybeOwnedInspector::Stacked(_) => {}
        }
    }
}
