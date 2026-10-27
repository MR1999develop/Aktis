"""Minimal reading and writing of ONNX files, without the onnx package.

An ONNX file is a protobuf message. This module knows only what the an earlier
denoiser tools need: the generic protobuf wire format (fields of a message
read and written in order) and builders for the few ONNX messages they
create (model, graph, node, tensor, attribute, value info).

Field numbers come from onnx.proto (ModelProto, GraphProto, NodeProto,
TensorProto, AttributeProto, ValueInfoProto); if a tool needs another field,
add its number here, next to the others.
"""
from __future__ import annotations

from typing import Final, Union

import numpy as np

# TensorProto.DataType
FLOAT: Final = 1
INT64: Final = 7
FLOAT16: Final = 10
NP_TYPE: Final = {FLOAT: "<f4", INT64: "<i8", FLOAT16: "<f2"}

# AttributeProto.AttributeType
ATTR_INT: Final = 2
ATTR_INTS: Final = 7

# a protobuf field: (field number, wire type, value: int for varints, bytes otherwise)
Field = tuple[int, int, Union[int, bytes]]


# ------------------------------------------------------------ wire format

def _read_varint(buf: bytes, pos: int) -> tuple[int, int]:
    result = shift = 0
    while True:
        b = buf[pos]
        pos += 1
        result |= (b & 0x7F) << shift
        if not b & 0x80:
            return result, pos
        shift += 7


def parse(buf: bytes) -> list[Field]:
    """Message → list of (field, wire type, value) in file order."""
    out: list[Field] = []
    pos = 0
    while pos < len(buf):
        key, pos = _read_varint(buf, pos)
        field, wire = key >> 3, key & 7
        val: int | bytes
        if wire == 0:
            val, pos = _read_varint(buf, pos)
        elif wire == 2:
            n, pos = _read_varint(buf, pos)
            val = buf[pos:pos + n]
            pos += n
        elif wire == 1:
            val = buf[pos:pos + 8]
            pos += 8
        elif wire == 5:
            val = buf[pos:pos + 4]
            pos += 4
        else:
            raise ValueError(f"unsupported protobuf wire type {wire}")
        out.append((field, wire, val))
    return out


def varint(v: int) -> bytes:
    """Protobuf varint; negative values are written as 64-bit two's complement."""
    v &= (1 << 64) - 1
    out = bytearray()
    while True:
        b = v & 0x7F
        v >>= 7
        if v:
            out.append(b | 0x80)
        else:
            out.append(b)
            return bytes(out)


def serialize(fields: list[Field]) -> bytes:
    out = bytearray()
    for field, wire, val in fields:
        out += varint((field << 3) | wire)
        if isinstance(val, int):
            out += varint(val)
        elif wire == 2:
            out += varint(len(val)) + val
        else:
            out += val
    return bytes(out)


def get(fields: list[Field], no: int) -> list:
    """Values of field `no`, in order (repeated fields give several)."""
    return [v for f, _, v in fields if f == no]


def signed(v: int) -> int:
    """A varint read back as a signed 64-bit integer (e.g. axis -1)."""
    return v - (1 << 64) if v >= 1 << 63 else v


def string(no: int, s: str) -> Field:
    return (no, 2, s.encode())


def message(no: int, body: bytes) -> Field:
    return (no, 2, body)


# ------------------------------------------------------------ ONNX messages

def attr_int(name: str, value: int) -> bytes:
    return serialize([string(1, name), (3, 0, value), (20, 0, ATTR_INT)])


def attr_ints(name: str, values: list[int]) -> bytes:
    return serialize([string(1, name)] + [(8, 0, v) for v in values] + [(20, 0, ATTR_INTS)])


def node(op: str, inputs: list[str], output: str, attrs: list[bytes] = (), *, name: str | None = None) -> bytes:
    """A node with one output; its name is the output's unless given (names
    must be unique in a graph)."""
    fields = [string(1, i) for i in inputs] + [string(2, output), string(3, name or output), string(4, op)]
    return serialize(fields + [message(5, a) for a in attrs])


def tensor(name: str, arr: np.ndarray, data_type: int) -> bytes:
    raw = np.ascontiguousarray(arr).astype(NP_TYPE[data_type]).tobytes()
    fields = [(1, 0, int(d)) for d in arr.shape] + [(2, 0, data_type), string(8, name), (9, 2, raw)]
    return serialize(fields)


def value_info(name: str, shape: list[int], elem_type: int = FLOAT) -> bytes:
    dims = b"".join(serialize([message(1, serialize([(1, 0, d)]))]) for d in shape)
    tensor_type = serialize([(1, 0, elem_type), message(2, dims)])
    return serialize([string(1, name), message(2, serialize([message(1, tensor_type)]))])


def model(nodes: list[bytes], initializers: list[bytes], inputs: list[bytes], outputs: list[bytes],
          *, graph_name: str, producer: str, opset: int) -> bytes:
    graph = serialize([message(1, n) for n in nodes] + [string(2, graph_name)]
                      + [message(5, t) for t in initializers]
                      + [message(11, i) for i in inputs] + [message(12, o) for o in outputs])
    return serialize([(1, 0, 7), string(2, producer), message(7, graph),
                      message(8, serialize([string(1, ""), (2, 0, opset)]))])
