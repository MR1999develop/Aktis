//! A safe interpreter for the pickle of a PyTorch checkpoint.
//!
//! A pickle is a small stack program that may call any Python function it
//! names. This interpreter knows only the handful of names a state dict
//! uses (`collections.OrderedDict`, `torch._utils._rebuild_tensor_v2`,
//! `torch._utils._rebuild_parameter` and the `torch.*Storage` types) and
//! only the opcodes `torch.save` emits for one; anything else is an error.
//! It mirrors `scripts/scunet/pth_weights.py`.
//!
//! Objects live in an arena and the stack holds their indices, so the memo
//! and nested containers share objects without reference counting, and a
//! dictionary filled after being memoised is seen filled everywhere.

use std::collections::{BTreeMap, HashMap};

use crate::zip::Archive;
use crate::{ConvertError, Tensor};

fn err(msg: impl Into<String>) -> ConvertError {
    ConvertError::Pickle(msg.into())
}

/// Element types of the storages a checkpoint may hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dtype {
    F32,
    F16,
    F64,
    I64,
    I32,
}

impl Dtype {
    fn from_storage(name: &str) -> Option<Self> {
        Some(match name {
            "FloatStorage" => Self::F32,
            "HalfStorage" => Self::F16,
            "DoubleStorage" => Self::F64,
            "LongStorage" => Self::I64,
            "IntStorage" => Self::I32,
            _ => return None,
        })
    }

    fn size(self) -> usize {
        match self {
            Self::F16 => 2,
            Self::F32 | Self::I32 => 4,
            Self::F64 | Self::I64 => 8,
        }
    }

    /// Element `i` of `raw` as f32 (the checkpoint's floats become f32 as in
    /// the Python reader; integers are converted too, rounding beyond 2^24).
    fn read(self, raw: &[u8], i: usize) -> f32 {
        let at = i * self.size();
        let b = &raw[at..at + self.size()];
        match self {
            Self::F32 => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            Self::F16 => f16_to_f32(u16::from_le_bytes([b[0], b[1]])),
            #[allow(clippy::cast_possible_truncation, reason = "rounds to the nearest f32, like NumPy's astype")]
            Self::F64 => f64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]) as f32,
            #[allow(clippy::cast_precision_loss, reason = "integer tensors are not used by the network")]
            Self::I64 => i64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]) as f32,
            #[allow(clippy::cast_precision_loss, reason = "integer tensors are not used by the network")]
            Self::I32 => i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32,
        }
    }
}

/// IEEE half precision → single precision (exact).
fn f16_to_f32(h: u16) -> f32 {
    let sign = u32::from(h >> 15) << 31;
    let exp = u32::from((h >> 10) & 0x1F);
    let man = u32::from(h & 0x3FF);
    let bits = match (exp, man) {
        (0, 0) => sign,
        (0, _) => {
            // subnormal: normalise the mantissa
            let shift = man.leading_zeros() - 21;
            sign | ((113 - shift) << 23) | (((man << shift) & 0x3FF) << 13)
        }
        (0x1F, _) => sign | 0x7F80_0000 | (man << 13),
        _ => sign | ((exp + 112) << 23) | (man << 13),
    };
    f32::from_bits(bits)
}

/// A tensor as the checkpoint stores it: a strided view of a storage,
/// checked to lie inside it. Weights stay views until they are needed, so
/// a conversion holds the checkpoint and the model, not a third copy.
#[derive(Debug, Clone)]
pub(crate) struct View<'a> {
    dtype: Dtype,
    raw: &'a [u8],
    offset: usize,
    shape: Vec<usize>,
    strides: Vec<usize>,
}

impl<'a> View<'a> {
    /// A contiguous f32 tensor stored in `raw`.
    #[cfg(test)]
    pub(crate) fn contiguous_f32(raw: &'a [u8], shape: &[usize]) -> Self {
        Self { dtype: Dtype::F32, raw, offset: 0, shape: shape.to_vec(), strides: row_major_strides(shape) }
    }

    /// Number of elements (1 for a scalar).
    pub(crate) fn numel(&self) -> usize {
        self.shape.iter().product()
    }

    /// The elements of the view, copied into a contiguous f32 tensor like
    /// NumPy's `np.array(as_strided(...))` in `pth_weights.py`.
    pub(crate) fn to_tensor(&self) -> Result<Tensor, ConvertError> {
        let Self { dtype, raw, offset, ref shape, ref strides } = *self;
        let numel = self.numel();
        let mut data = Vec::new();
        data.try_reserve_exact(numel).map_err(|_| err("tensor too large for memory"))?;
        if shape.is_empty() || row_major_strides(shape) == *strides {
            data.extend((offset..offset + numel).map(|i| dtype.read(raw, i)));
        } else if numel > 0 {
            // walk the multi-index in row-major order
            let mut index = vec![0usize; shape.len()];
            let mut at = offset;
            for _ in 0..numel {
                data.push(dtype.read(raw, at));
                // wrapping: a size-1 axis may have any stride, and the
                // position comes back in range when the axis wraps
                for ax in (0..shape.len()).rev() {
                    index[ax] += 1;
                    at = at.wrapping_add(strides[ax]);
                    if index[ax] < shape[ax] {
                        break;
                    }
                    at = at.wrapping_sub(strides[ax].wrapping_mul(shape[ax]));
                    index[ax] = 0;
                }
            }
        }
        Ok(Tensor { shape: shape.clone(), data })
    }
}

/// The Python functions and classes a checkpoint may name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Global {
    OrderedDict,
    RebuildTensor,
    RebuildParameter,
    Storage(Dtype),
}

/// A dictionary key; checkpoints use strings (and integers in metadata).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Key {
    Str(String),
    Int(i64),
}

#[derive(Debug, Default)]
struct Dict {
    entries: Vec<(Key, usize)>,
    index: HashMap<Key, usize>,
}

impl Dict {
    /// Python semantics: a new key goes at the end, an existing one keeps
    /// its place and gets the new value.
    fn insert(&mut self, key: Key, value: usize) {
        if let Some(&i) = self.index.get(&key) {
            self.entries[i].1 = value;
        } else {
            self.index.insert(key.clone(), self.entries.len());
            self.entries.push((key, value));
        }
    }

    fn get(&self, key: &str) -> Option<usize> {
        self.index.get(&Key::Str(key.to_owned())).map(|&i| self.entries[i].1)
    }
}

#[derive(Debug)]
enum Obj<'a> {
    None,
    Bool(bool),
    Int(i64),
    Str(String),
    Tuple(Vec<usize>),
    Dict(Dict),
    Global(Global),
    Storage { dtype: Dtype, raw: &'a [u8] },
    Tensor(View<'a>),
}

impl Obj<'_> {
    fn kind(&self) -> &'static str {
        match self {
            Obj::None => "None",
            Obj::Bool(_) => "bool",
            Obj::Int(_) => "int",
            Obj::Str(_) => "str",
            Obj::Tuple(_) => "tuple",
            Obj::Dict(_) => "dict",
            Obj::Global(_) => "class",
            Obj::Storage { .. } => "storage",
            Obj::Tensor(_) => "tensor",
        }
    }
}

struct Machine<'a, 'z> {
    archive: &'z Archive<'a>,
    /// Folder of the checkpoint inside the zip (`archive` for torch.save).
    root: &'z str,
    code: &'z [u8],
    pos: usize,
    objs: Vec<Obj<'a>>,
    stack: Vec<usize>,
    marks: Vec<usize>,
    memo: HashMap<u32, usize>,
}

impl<'a, 'z> Machine<'a, 'z> {
    fn take(&mut self, n: usize) -> Result<&'z [u8], ConvertError> {
        let end = self.pos.checked_add(n).filter(|&e| e <= self.code.len()).ok_or_else(|| err("truncated pickle"))?;
        let s = &self.code[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], ConvertError> {
        let mut a = [0; N];
        a.copy_from_slice(self.take(N)?);
        Ok(a)
    }

    fn line(&mut self) -> Result<&'z str, ConvertError> {
        let rest = &self.code[self.pos..];
        let n = rest.iter().position(|&b| b == b'\n').ok_or_else(|| err("truncated pickle"))?;
        let s = std::str::from_utf8(&rest[..n]).map_err(|_| err("bad GLOBAL name"))?;
        self.pos += n + 1;
        Ok(s)
    }

    fn push(&mut self, obj: Obj<'a>) {
        self.stack.push(self.objs.len());
        self.objs.push(obj);
    }

    fn pop(&mut self) -> Result<usize, ConvertError> {
        // never pop below the last mark: those items belong to it
        if self.stack.len() <= self.marks.last().copied().unwrap_or(0) {
            return Err(err("stack underflow"));
        }
        self.stack.pop().ok_or_else(|| err("stack underflow"))
    }

    fn top(&self) -> Result<usize, ConvertError> {
        if self.stack.len() <= self.marks.last().copied().unwrap_or(0) {
            return Err(err("stack underflow"));
        }
        self.stack.last().copied().ok_or_else(|| err("stack underflow"))
    }

    /// The items pushed since the last MARK, which is removed.
    fn pop_mark(&mut self) -> Result<Vec<usize>, ConvertError> {
        let m = self.marks.pop().ok_or_else(|| err("missing MARK"))?;
        Ok(self.stack.split_off(m))
    }

    fn str_of(&self, id: usize) -> Result<&str, ConvertError> {
        match &self.objs[id] {
            Obj::Str(s) => Ok(s),
            o => Err(err(format!("expected a string, found {}", o.kind()))),
        }
    }

    fn int_of(&self, id: usize) -> Result<i64, ConvertError> {
        match self.objs[id] {
            Obj::Int(v) => Ok(v),
            Obj::Bool(b) => Ok(i64::from(b)),
            ref o => Err(err(format!("expected an integer, found {}", o.kind()))),
        }
    }

    fn usize_of(&self, id: usize) -> Result<usize, ConvertError> {
        usize::try_from(self.int_of(id)?).map_err(|_| err("negative or too large tensor geometry"))
    }

    fn tuple_of(&self, id: usize) -> Result<&[usize], ConvertError> {
        match &self.objs[id] {
            Obj::Tuple(t) => Ok(t),
            o => Err(err(format!("expected a tuple, found {}", o.kind()))),
        }
    }

    fn key_of(&self, id: usize) -> Result<Key, ConvertError> {
        match &self.objs[id] {
            Obj::Str(s) => Ok(Key::Str(s.clone())),
            Obj::Int(v) => Ok(Key::Int(*v)),
            o => Err(err(format!("unsupported dictionary key of type {}", o.kind()))),
        }
    }

    fn set_items(&mut self, dict: usize, items: &[usize]) -> Result<(), ConvertError> {
        if !items.len().is_multiple_of(2) {
            return Err(err("odd number of items for SETITEMS"));
        }
        let pairs = items
            .as_chunks::<2>()
            .0
            .iter()
            .map(|kv| Ok((self.key_of(kv[0])?, kv[1])))
            .collect::<Result<Vec<_>, _>>()?;
        match &mut self.objs[dict] {
            Obj::Dict(d) => {
                for (k, v) in pairs {
                    d.insert(k, v);
                }
                Ok(())
            }
            o => Err(err(format!("SETITEM on a {}", o.kind()))),
        }
    }

    fn global(module: &str, name: &str) -> Result<Global, ConvertError> {
        let g = match (module, name) {
            ("collections", "OrderedDict") => Some(Global::OrderedDict),
            ("torch._utils", "_rebuild_tensor_v2") => Some(Global::RebuildTensor),
            ("torch._utils", "_rebuild_parameter") => Some(Global::RebuildParameter),
            ("torch", n) => Dtype::from_storage(n).map(Global::Storage),
            _ => None,
        };
        g.ok_or_else(|| err(format!("unexpected object in the checkpoint: {module}.{name}")))
    }

    /// BINPERSID: ('storage', <storage class>, key, location, numel) → the
    /// raw bytes of `<root>/data/<key>`.
    fn persistent_load(&mut self, pid: usize) -> Result<Obj<'a>, ConvertError> {
        let t = self.tuple_of(pid)?;
        let [kind, class, key, _location, _numel] = t else {
            return Err(err("unexpected persistent id"));
        };
        if self.str_of(*kind)? != "storage" {
            return Err(err("unexpected persistent id"));
        }
        let Obj::Global(Global::Storage(dtype)) = self.objs[*class] else {
            return Err(err("unsupported storage type"));
        };
        let raw = self.archive.read(&format!("{}/data/{}", self.root, self.str_of(*key)?))?;
        if !raw.len().is_multiple_of(dtype.size()) {
            return Err(err("storage size is not a multiple of its element size"));
        }
        Ok(Obj::Storage { dtype, raw })
    }

    fn reduce(&mut self, func: usize, args: usize) -> Result<(), ConvertError> {
        let Obj::Global(g) = self.objs[func] else {
            return Err(err(format!("REDUCE on a {}", self.objs[func].kind())));
        };
        let args = self.tuple_of(args)?.to_vec();
        match g {
            Global::OrderedDict if args.is_empty() => self.push(Obj::Dict(Dict::default())),
            Global::RebuildTensor => {
                let tensor = self.rebuild_tensor(&args)?;
                self.push(Obj::Tensor(tensor));
            }
            Global::RebuildParameter => {
                // a parameter is its data tensor (the same object)
                let data = *args.first().ok_or_else(|| err("_rebuild_parameter without data"))?;
                if !matches!(self.objs[data], Obj::Tensor(_)) {
                    return Err(err("_rebuild_parameter of a non-tensor"));
                }
                self.stack.push(data);
            }
            _ => return Err(err(format!("unsupported call of {g:?}"))),
        }
        Ok(())
    }

    /// `_rebuild_tensor_v2(storage, offset, size, stride, …)`: the strided
    /// view, checked to lie inside its storage; it is copied out later.
    fn rebuild_tensor(&self, args: &[usize]) -> Result<View<'a>, ConvertError> {
        let [storage, offset, size, stride, ..] = args else {
            return Err(err("_rebuild_tensor_v2 with too few arguments"));
        };
        let Obj::Storage { dtype, raw } = self.objs[*storage] else {
            return Err(err("tensor without a storage"));
        };
        let offset = self.usize_of(*offset)?;
        let shape = self.tuple_of(*size)?.iter().map(|&i| self.usize_of(i)).collect::<Result<Vec<_>, _>>()?;
        let strides = self.tuple_of(*stride)?.iter().map(|&i| self.usize_of(i)).collect::<Result<Vec<_>, _>>()?;
        let numel_storage = raw.len() / dtype.size();
        if shape.is_empty() {
            // a scalar: element `offset` (the stride tuple is ignored)
            if offset >= numel_storage {
                return Err(err("scalar tensor outside its storage"));
            }
            return Ok(View { dtype, raw, offset, shape, strides: Vec::new() });
        }
        if strides.len() != shape.len() {
            return Err(err("tensor size and stride differ in length"));
        }
        let overflow = || err("tensor geometry overflows");
        let numel = shape.iter().try_fold(1usize, |a, &d| a.checked_mul(d)).ok_or_else(overflow)?;
        if numel > 0 {
            let mut last = offset;
            for (&d, &s) in shape.iter().zip(&strides) {
                last = (d - 1).checked_mul(s).and_then(|x| x.checked_add(last)).ok_or_else(overflow)?;
            }
            if last >= numel_storage {
                return Err(err("tensor reaches outside its storage"));
            }
        }
        Ok(View { dtype, raw, offset, shape, strides })
    }

    fn run(&mut self) -> Result<usize, ConvertError> {
        loop {
            let [op] = self.array()?;
            match op {
                // PROTO: versions 2 to 5 share the opcodes used here
                0x80 => {
                    let [v] = self.array()?;
                    if !(2..=5).contains(&v) {
                        return Err(err(format!("unsupported pickle protocol {v}")));
                    }
                }
                // FRAME (protocol 4+): only a length hint
                0x95 => {
                    self.take(8)?;
                }
                b'.' => return self.pop(),                 // STOP
                b'(' => self.marks.push(self.stack.len()), // MARK
                b'N' => self.push(Obj::None),
                0x88 => self.push(Obj::Bool(true)),
                0x89 => self.push(Obj::Bool(false)),
                b'J' => {
                    let v = i32::from_le_bytes(self.array()?);
                    self.push(Obj::Int(i64::from(v)));
                }
                b'K' => {
                    let [v] = self.array()?;
                    self.push(Obj::Int(i64::from(v)));
                }
                b'M' => {
                    let v = u16::from_le_bytes(self.array()?);
                    self.push(Obj::Int(i64::from(v)));
                }
                // LONG1: little-endian two's complement of n bytes
                0x8a => {
                    let [n] = self.array()?;
                    let bytes = self.take(usize::from(n))?;
                    if n > 8 {
                        return Err(err("integer too large"));
                    }
                    let fill = if bytes.last().is_some_and(|&b| b & 0x80 != 0) { 0xFF } else { 0 };
                    let mut b = [fill; 8];
                    b[..bytes.len()].copy_from_slice(bytes);
                    self.push(Obj::Int(i64::from_le_bytes(b)));
                }
                // BINUNICODE, SHORT_BINUNICODE
                b'X' | 0x8c => {
                    let n = if op == b'X' {
                        usize::try_from(u32::from_le_bytes(self.array()?)).map_err(|_| err("string too long"))?
                    } else {
                        usize::from(self.array::<1>()?[0])
                    };
                    let s = std::str::from_utf8(self.take(n)?).map_err(|_| err("invalid UTF-8 string"))?;
                    self.push(Obj::Str(s.to_owned()));
                }
                b')' => self.push(Obj::Tuple(Vec::new())),
                b't' => {
                    let items = self.pop_mark()?;
                    self.push(Obj::Tuple(items));
                }
                0x85..=0x87 => {
                    let n = usize::from(op - 0x84);
                    let mut items = Vec::with_capacity(n);
                    for _ in 0..n {
                        items.push(self.pop()?);
                    }
                    items.reverse();
                    self.push(Obj::Tuple(items));
                }
                b'}' => self.push(Obj::Dict(Dict::default())),
                b's' => {
                    let v = self.pop()?;
                    let k = self.pop()?;
                    let d = self.top()?;
                    self.set_items(d, &[k, v])?;
                }
                b'u' => {
                    let items = self.pop_mark()?;
                    let d = self.top()?;
                    self.set_items(d, &items)?;
                }
                b'c' => {
                    let module = self.line()?;
                    let name = self.line()?;
                    let g = Self::global(module, name)?;
                    self.push(Obj::Global(g));
                }
                // STACK_GLOBAL (protocol 4+)
                0x93 => {
                    let name = self.pop()?;
                    let module = self.pop()?;
                    let g = Self::global(self.str_of(module)?, self.str_of(name)?)?;
                    self.push(Obj::Global(g));
                }
                b'R' => {
                    let args = self.pop()?;
                    let func = self.pop()?;
                    self.reduce(func, args)?;
                }
                b'Q' => {
                    let pid = self.pop()?;
                    let storage = self.persistent_load(pid)?;
                    self.push(storage);
                }
                // BUILD on an OrderedDict sets attributes (torch stores the
                // module versions in `_metadata`); they are not weights.
                b'b' => {
                    let state = self.pop()?;
                    let target = self.top()?;
                    if !matches!((&self.objs[target], &self.objs[state]), (Obj::Dict(_), Obj::Dict(_) | Obj::None)) {
                        return Err(err(format!("BUILD on a {}", self.objs[target].kind())));
                    }
                }
                b'q' | b'r' => {
                    let i =
                        if op == b'q' { u32::from(self.array::<1>()?[0]) } else { u32::from_le_bytes(self.array()?) };
                    let top = self.top()?;
                    self.memo.insert(i, top);
                }
                // MEMOIZE (protocol 4+): the next free memo slot
                0x94 => {
                    let i = u32::try_from(self.memo.len()).map_err(|_| err("memo too large"))?;
                    let top = self.top()?;
                    self.memo.insert(i, top);
                }
                b'h' | b'j' => {
                    let i =
                        if op == b'h' { u32::from(self.array::<1>()?[0]) } else { u32::from_le_bytes(self.array()?) };
                    let id = *self.memo.get(&i).ok_or_else(|| err(format!("memo {i} is empty")))?;
                    self.stack.push(id);
                }
                _ => return Err(err(format!("unsupported pickle opcode 0x{op:02x} at byte {}", self.pos - 1))),
            }
        }
    }

    /// The tensors of the dictionary `id` by name.
    fn into_views(self, id: usize) -> Result<BTreeMap<String, View<'a>>, ConvertError> {
        let Obj::Dict(dict) = &self.objs[id] else {
            return Err(err("the checkpoint does not contain a dictionary of weights"));
        };
        let mut out = BTreeMap::new();
        for (key, value) in &dict.entries {
            let Key::Str(name) = key else {
                return Err(err("a weight name is not a string"));
            };
            let Obj::Tensor(view) = &self.objs[*value] else {
                return Err(err(format!("{name} is not a tensor")));
            };
            out.insert(name.clone(), view.clone());
        }
        Ok(out)
    }
}

/// Row-major (C order) strides of `shape`, in elements.
fn row_major_strides(shape: &[usize]) -> Vec<usize> {
    let mut strides = vec![1usize; shape.len()];
    for i in (0..shape.len().saturating_sub(1)).rev() {
        strides[i] = strides[i + 1].saturating_mul(shape[i + 1]);
    }
    strides
}

/// The weights of a zip checkpoint, as `pth_weights.load_state_dict`.
pub(crate) fn load(pth: &[u8]) -> Result<BTreeMap<String, Tensor>, ConvertError> {
    load_views(pth)?.into_iter().map(|(name, v)| Ok((name, v.to_tensor()?))).collect()
}

/// The weights of a zip checkpoint as views into `pth`.
pub(crate) fn load_views(pth: &[u8]) -> Result<BTreeMap<String, View<'_>>, ConvertError> {
    let archive = Archive::new(pth)?;
    let pkl = archive
        .names()
        .find(|n| n.ends_with("/data.pkl"))
        .ok_or_else(|| ConvertError::Archive("no data.pkl: not a PyTorch checkpoint".into()))?
        .to_owned();
    let root = &pkl[..pkl.len() - "/data.pkl".len()];
    let code = archive.read(&pkl)?;
    let mut m = Machine {
        archive: &archive,
        root,
        code,
        pos: 0,
        objs: Vec::new(),
        stack: Vec::new(),
        marks: Vec::new(),
        memo: HashMap::new(),
    };
    let mut obj = m.run()?;
    // some checkpoints wrap the weights: {"params": {...}} and the like
    for key in ["params", "state_dict", "model"] {
        if let Obj::Dict(d) = &m.objs[obj]
            && let Some(inner) = d.get(key)
            && matches!(m.objs[inner], Obj::Dict(_))
        {
            obj = inner;
        }
    }
    m.into_views(obj)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::float_cmp)]
mod tests {
    use super::*;
    use crate::zip::tests::make_zip;

    /// Builds pickle programs in the shape `torch.save` writes them.
    #[derive(Default)]
    struct P(Vec<u8>);

    impl P {
        fn op(&mut self, b: &[u8]) -> &mut Self {
            self.0.extend_from_slice(b);
            self
        }
        fn s(&mut self, s: &str) -> &mut Self {
            self.0.push(b'X');
            self.0.extend_from_slice(&u32::try_from(s.len()).unwrap().to_le_bytes());
            self.op(s.as_bytes())
        }
        fn int(&mut self, v: u16) -> &mut Self {
            if v < 256 { self.op(&[b'K', v.to_le_bytes()[0]]) } else { self.op(b"M").op(&v.to_le_bytes()) }
        }
        fn ints(&mut self, v: &[u16]) -> &mut Self {
            self.op(b"(");
            for &x in v {
                self.int(x);
            }
            self.op(b"t")
        }
        /// `name: _rebuild_tensor_v2(storage, offset, size, stride, False, OrderedDict())`,
        /// optionally wrapped in `_rebuild_parameter`.
        fn tensor(&mut self, storage: &str, key: &str, offset: u16, size: &[u16], stride: &[u16], param: bool) {
            if param {
                self.op(b"ctorch._utils\n_rebuild_parameter\n(");
            }
            self.op(b"ctorch._utils\n_rebuild_tensor_v2\n((").s("storage");
            self.op(format!("ctorch\n{storage}\n").as_bytes()).s(key).s("cpu").int(0).op(b"tQ");
            self.int(offset).ints(size).ints(stride);
            self.op(&[0x89]).op(b"ccollections\nOrderedDict\n)RtR");
            if param {
                self.op(&[0x88]).op(b"ccollections\nOrderedDict\n)RtR");
            }
        }
    }

    fn f32s(v: &[f32]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_le_bytes()).collect()
    }

    fn checkpoint(p: &P, storages: &[(&str, &[u8])]) -> Vec<u8> {
        let mut files: Vec<(String, &[u8])> = vec![("archive/data.pkl".into(), p.0.as_slice())];
        files.extend(storages.iter().map(|(k, d)| (format!("archive/data/{k}"), *d)));
        let refs: Vec<(&str, &[u8])> = files.iter().map(|(n, d)| (n.as_str(), *d)).collect();
        make_zip(&refs, 0)
    }

    #[test]
    fn half_to_single() {
        assert_eq!(f16_to_f32(0x3C00), 1.0);
        assert_eq!(f16_to_f32(0xC000), -2.0);
        assert_eq!(f16_to_f32(0x7BFF), 65504.0);
        assert_eq!(f16_to_f32(0x0001), 2f32.powi(-24));
        assert_eq!(f16_to_f32(0x03FF), 1023.0 * 2f32.powi(-24));
        assert_eq!(f16_to_f32(0x7C00), f32::INFINITY);
        assert!(f16_to_f32(0x7E00).is_nan());
        assert_eq!(f16_to_f32(0x8000).to_bits(), (-0.0f32).to_bits());
    }

    /// A state dict laid out like the real one: an OrderedDict filled with
    /// SETITEMS, parameters, a transposed (non-contiguous) tensor, a scalar,
    /// a half tensor, memo references and the `_metadata` BUILD.
    #[test]
    fn reads_a_state_dict() {
        let w = load(&sample_checkpoint()).unwrap();
        assert_eq!(w.keys().collect::<Vec<_>>(), ["a", "b", "c", "d"]);
        assert_eq!(w["a"], Tensor { shape: vec![2, 3], data: vec![1., 2., 3., 4., 5., 6.] });
        // b[i][j] = storage[i + 2j]
        assert_eq!(w["b"], Tensor { shape: vec![2, 3], data: vec![0., 2., 4., 1., 3., 5.] });
        assert_eq!(w["c"], Tensor { shape: vec![], data: vec![1.0] });
        assert_eq!(w["d"], w["c"]);
    }

    #[test]
    fn the_fuzzers_start_from_a_checkpoint_like_the_real_one() {
        mutate::check_seed("checkpoint", "state_dict.pth", &sample_checkpoint());
    }

    #[test]
    fn damaged_checkpoints_never_panic() {
        let (program, storages) = sample_parts();
        let refs: Vec<(&str, &[u8])> = storages.iter().map(|(k, d)| (*k, d.as_slice())).collect();
        // The pickle program damaged inside an intact zip, then the zip itself.
        for copy in mutate::copies(&program.0, 51, 4000) {
            let _ = load(&checkpoint(&P(copy), &refs));
        }
        for copy in mutate::copies(&sample_checkpoint(), 52, 3000) {
            let _ = load(&copy);
        }
    }

    /// The state dict of [`reads_a_state_dict`].
    fn sample_checkpoint() -> Vec<u8> {
        let (p, storages) = sample_parts();
        let refs: Vec<(&str, &[u8])> = storages.iter().map(|(k, d)| (*k, d.as_slice())).collect();
        checkpoint(&p, &refs)
    }

    /// Its pickle program and storages.
    fn sample_parts() -> (P, Vec<(&'static str, Vec<u8>)>) {
        let mut p = P::default();
        p.op(&[0x80, 2]).op(b"ccollections\nOrderedDict\nq\x00)Rq\x01(");
        // a: 2×3 contiguous, from offset 1 of storage "0"
        p.s("a");
        p.tensor("FloatStorage", "0", 1, &[2, 3], &[3, 1], false);
        // b: the transpose of a 3×2 matrix stored at offset 0 (strides 1, 3),
        // like relative_position_params
        p.s("b");
        p.tensor("FloatStorage", "0", 0, &[2, 3], &[1, 2], true);
        p.s("c");
        p.tensor("HalfStorage", "1", 1, &[], &[], false);
        p.op(b"q\x02");
        p.s("d").op(b"h\x02"); // the same tensor under a second name
        p.op(b"u}q\x03").s("_metadata").op(b"h\x00)R(").s("").op(b"}").s("version").int(1).op(b"sus");
        p.op(b"b.");
        let storage0 = f32s(&[0., 1., 2., 3., 4., 5., 6.]);
        let storage1: Vec<u8> = [0x0000u16, 0x3C00, 0xC000].iter().flat_map(|h| h.to_le_bytes()).collect();
        (p, vec![("0", storage0), ("1", storage1)])
    }

    #[test]
    fn unwraps_nested_state_dicts() {
        let mut p = P::default();
        p.op(&[0x80, 2]).op(b"}(").s("state_dict").op(b"ccollections\nOrderedDict\n)R(").s("w");
        p.tensor("DoubleStorage", "0", 0, &[2], &[1], false);
        p.op(b"uu.");
        let storage: Vec<u8> = [0.5f64, -1.25].iter().flat_map(|x| x.to_le_bytes()).collect();
        let w = load(&checkpoint(&p, &[("0", &storage)])).unwrap();
        assert_eq!(w["w"].data, [0.5, -1.25]);
    }

    #[test]
    fn refuses_foreign_objects_and_bad_geometry() {
        let msg = |p: &P, s: &[u8]| load(&checkpoint(p, &[("0", s)])).unwrap_err().to_string();
        let mut p = P::default();
        p.op(&[0x80, 2]).op(b"cos\nsystem\n").s("echo hi").op(b"\x85R.");
        assert!(msg(&p, b"").contains("os.system"));

        let mut p = P::default();
        p.op(&[0x80, 2]).op(b"}(").s("w");
        p.tensor("FloatStorage", "0", 2, &[2], &[1], false);
        p.op(b"u.");
        assert!(msg(&p, &f32s(&[1., 2., 3.])).contains("outside"));

        let mut p = P::default();
        p.op(&[0x80, 2]).op(b"}(").s("w").int(3).op(b"u.");
        assert!(msg(&p, b"").contains("not a tensor"));

        let mut p = P::default();
        p.op(&[0x80, 2]).op(b"]."); // EMPTY_LIST is not used by checkpoints
        assert!(msg(&p, b"").contains("opcode 0x5d"));

        let mut p = P::default();
        p.op(&[0x80, 2]).op(b"}(").s("w");
        p.tensor("BFloat16Storage", "0", 0, &[1], &[1], false);
        p.op(b"u.");
        assert!(msg(&p, b"\0\0").contains("BFloat16Storage"));

        // every truncation of a valid program fails cleanly
        let mut p = P::default();
        p.op(&[0x80, 2]).op(b"}(").s("w");
        p.tensor("FloatStorage", "0", 0, &[1], &[1], true);
        p.op(b"u.");
        let s = f32s(&[7.0]);
        assert_eq!(load(&checkpoint(&p, &[("0", &s)])).unwrap()["w"].data, [7.0]);
        for n in 0..p.0.len() {
            let cut = P(p.0[..n].to_vec());
            assert!(load(&checkpoint(&cut, &[("0", &s)])).is_err());
        }
    }
}
