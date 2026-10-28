//! Writing of the few ONNX protobuf messages the model needs, byte for byte
//! as `scripts/scunet/onnxproto.py` writes them (fields in the same order,
//! repeated integers unpacked, empty strings written out).
//!
//! Field numbers come from onnx.proto (ModelProto, GraphProto, NodeProto,
//! TensorProto, AttributeProto, ValueInfoProto).

/// TensorProto.DataType FLOAT.
pub(crate) const FLOAT: u64 = 1;
/// TensorProto.DataType INT64.
pub(crate) const INT64: u64 = 7;
/// AttributeProto.AttributeType INT.
const ATTR_INT: u64 = 2;
/// AttributeProto.AttributeType INTS.
const ATTR_INTS: u64 = 7;

const WIRE_VARINT: u64 = 0;
const WIRE_LEN: u64 = 2;

/// Appends a protobuf varint.
pub(crate) fn varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push(v.to_le_bytes()[0] | 0x80);
        v >>= 7;
    }
    out.push(v.to_le_bytes()[0]);
}

/// A varint field; negative values are written as 64-bit two's complement.
pub(crate) fn int_field(out: &mut Vec<u8>, field: u64, v: i64) {
    varint(out, field << 3 | WIRE_VARINT);
    varint(out, v.cast_unsigned());
}

/// A length-delimited field (string, bytes or embedded message).
pub(crate) fn bytes_field(out: &mut Vec<u8>, field: u64, body: &[u8]) {
    len_prefix(out, field, body.len());
    out.extend_from_slice(body);
}

/// The key and length of a length-delimited field whose body follows.
pub(crate) fn len_prefix(out: &mut Vec<u8>, field: u64, len: usize) {
    varint(out, field << 3 | WIRE_LEN);
    varint(out, len as u64);
}

/// AttributeProto with an integer value.
pub(crate) fn attr_int(name: &str, value: i64) -> Vec<u8> {
    let mut out = Vec::new();
    bytes_field(&mut out, 1, name.as_bytes());
    int_field(&mut out, 3, value);
    int_field(&mut out, 20, ATTR_INT.cast_signed());
    out
}

/// AttributeProto with a list of integers.
pub(crate) fn attr_ints(name: &str, values: &[i64]) -> Vec<u8> {
    let mut out = Vec::new();
    bytes_field(&mut out, 1, name.as_bytes());
    for &v in values {
        int_field(&mut out, 8, v);
    }
    int_field(&mut out, 20, ATTR_INTS.cast_signed());
    out
}

/// NodeProto with one output.
pub(crate) fn node(op: &str, inputs: &[&str], output: &str, name: &str, attrs: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    for i in inputs {
        bytes_field(&mut out, 1, i.as_bytes());
    }
    bytes_field(&mut out, 2, output.as_bytes());
    bytes_field(&mut out, 3, name.as_bytes());
    bytes_field(&mut out, 4, op.as_bytes());
    for a in attrs {
        bytes_field(&mut out, 5, a);
    }
    out
}

/// The fields of a TensorProto before its raw data: dims, data type, name
/// and the key and length of `raw_data`. The caller appends the
/// `raw_len` bytes, so large weights go straight to the output.
pub(crate) fn tensor_head(name: &str, shape: &[usize], data_type: u64, raw_len: usize) -> Vec<u8> {
    let mut out = Vec::new();
    for &d in shape {
        varint(&mut out, 1 << 3 | WIRE_VARINT);
        varint(&mut out, d as u64);
    }
    varint(&mut out, 2 << 3 | WIRE_VARINT);
    varint(&mut out, data_type);
    bytes_field(&mut out, 8, name.as_bytes());
    len_prefix(&mut out, 9, raw_len);
    out
}

/// ValueInfoProto of a tensor with a fixed shape.
pub(crate) fn value_info(name: &str, shape: &[usize], elem_type: u64) -> Vec<u8> {
    let mut dims = Vec::new();
    for &d in shape {
        let mut dim = Vec::new();
        varint(&mut dim, 1 << 3 | WIRE_VARINT);
        varint(&mut dim, d as u64);
        bytes_field(&mut dims, 1, &dim);
    }
    let mut tensor_type = Vec::new();
    varint(&mut tensor_type, 1 << 3 | WIRE_VARINT);
    varint(&mut tensor_type, elem_type);
    bytes_field(&mut tensor_type, 2, &dims);
    let mut type_proto = Vec::new();
    bytes_field(&mut type_proto, 1, &tensor_type);
    let mut out = Vec::new();
    bytes_field(&mut out, 1, name.as_bytes());
    bytes_field(&mut out, 2, &type_proto);
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    #[test]
    fn varints() {
        for (v, want) in
            [(0u64, "00"), (1, "01"), (127, "7f"), (128, "8001"), (300, "ac02"), (u64::MAX, "ffffffffffffffffff01")]
        {
            let mut out = Vec::new();
            varint(&mut out, v);
            assert_eq!(hex(&out), want);
        }
    }

    // The expected bytes below were printed by scripts/scunet/onnxproto.py.

    #[test]
    fn attributes_match_python() {
        // pb.attr_int("axis", -1)
        assert_eq!(hex(&attr_int("axis", -1)), "0a046178697318ffffffffffffffffff01a00102");
        // pb.attr_ints("perm", [0, 2, 3, 1])
        assert_eq!(hex(&attr_ints("perm", &[0, 2, 3, 1])), "0a047065726d4000400240034001a00107");
    }

    #[test]
    fn node_matches_python() {
        // pb.node("Slice", ["x", "c_1", "c_2"], "slice_3", [pb.attr_int("keepdims", 1)])
        let n = node("Slice", &["x", "c_1", "c_2"], "slice_3", "slice_3", &[attr_int("keepdims", 1)]);
        assert_eq!(
            hex(&n),
            "0a01780a03635f310a03635f321207736c6963655f331a07736c6963655f332205536c6963652a0f0a086b656570646\
             96d731801a00102"
        );
    }

    #[test]
    fn tensor_and_value_info_match_python() {
        // pb.tensor("w_1", np.array([[1.5, -2.0]], np.float32), pb.FLOAT)
        let mut t = tensor_head("w_1", &[1, 2], FLOAT, 8);
        t.extend([1.5f32, -2.0].iter().flat_map(|x| x.to_le_bytes()));
        assert_eq!(hex(&t), "0801080210014203775f314a080000c03f000000c0");
        // pb.value_info("input", [1, 3, 448, 448])
        assert_eq!(
            hex(&value_info("input", &[1, 3, 448, 448], FLOAT)),
            "0a05696e70757412180a16080112120a0208010a0208030a0308c0030a0308c003"
        );
    }
}
