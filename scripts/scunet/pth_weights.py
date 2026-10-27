"""Reads the weights of a PyTorch checkpoint (.pth) without PyTorch.

A modern checkpoint is a zip file: `<root>/data.pkl` is a pickle that
describes the tensors, and `<root>/data/<key>` holds the raw bytes of each
storage. The pickle names PyTorch functions to rebuild the tensors; here they
are replaced by functions that build NumPy arrays, and every other class is
refused, so loading a file cannot run arbitrary code.
"""
from __future__ import annotations

import collections
import io
import pickle
import zipfile
from pathlib import Path

import numpy as np

# storage classes of PyTorch → NumPy element type
_STORAGE_DTYPES = {
    "FloatStorage": "<f4",
    "HalfStorage": "<f2",
    "DoubleStorage": "<f8",
    "LongStorage": "<i8",
    "IntStorage": "<i4",
    "BFloat16Storage": None,  # not used by the SCUNet weights
}


class WeightsError(Exception):
    """The file is not a PyTorch checkpoint this reader understands."""


def _rebuild_tensor(storage, offset, size, stride, *_unused):
    data, = storage
    if not size:
        return data[offset:offset + 1].reshape(())
    itemsize = data.dtype.itemsize
    view = np.lib.stride_tricks.as_strided(data[offset:], shape=tuple(size),
                                           strides=tuple(s * itemsize for s in stride))
    return np.array(view)  # owned copy, contiguous


def _rebuild_parameter(data, *_unused):
    return data


class _Unpickler(pickle.Unpickler):
    def __init__(self, file, archive: zipfile.ZipFile, root: str):
        super().__init__(file)
        self._archive = archive
        self._root = root

    def find_class(self, module: str, name: str):
        allowed = {
            ("collections", "OrderedDict"): collections.OrderedDict,
            ("torch._utils", "_rebuild_tensor_v2"): _rebuild_tensor,
            ("torch._utils", "_rebuild_parameter"): _rebuild_parameter,
        }
        if (module, name) in allowed:
            return allowed[(module, name)]
        if module == "torch" and name in _STORAGE_DTYPES:
            return name  # the storage type travels as its name
        raise WeightsError(f"unexpected object in the checkpoint: {module}.{name}")

    def persistent_load(self, pid):
        kind, storage_type, key, _location, _numel = pid
        if kind != "storage":
            raise WeightsError(f"unexpected persistent id {kind!r}")
        dtype = _STORAGE_DTYPES.get(storage_type)
        if dtype is None:
            raise WeightsError(f"unsupported storage {storage_type}")
        raw = self._archive.read(f"{self._root}/data/{key}")
        return (np.frombuffer(raw, dtype=dtype),)


def load_state_dict(path: Path) -> dict[str, np.ndarray]:
    """Tensors of the checkpoint at `path`, by name, as float32 arrays
    (integer tensors keep their type).

    Raises WeightsError for files that are not zip checkpoints, or that
    contain anything other than tensors in (nested) dictionaries."""
    try:
        archive = zipfile.ZipFile(path)
    except zipfile.BadZipFile as e:
        raise WeightsError(f"{path.name} is not a PyTorch zip checkpoint") from e
    with archive:
        pkl = next((n for n in archive.namelist() if n.endswith("/data.pkl")), None)
        if pkl is None:
            raise WeightsError(f"{path.name} has no data.pkl")
        root = pkl[: -len("/data.pkl")]
        obj = _Unpickler(io.BytesIO(archive.read(pkl)), archive, root).load()
    # some checkpoints wrap the weights: {"params": {...}} or {"state_dict": {...}}
    for key in ("params", "state_dict", "model"):
        if isinstance(obj, dict) and key in obj and isinstance(obj[key], dict):
            obj = obj[key]
    if not isinstance(obj, dict):
        raise WeightsError("the checkpoint does not contain a dictionary of weights")
    out = {}
    for name, value in obj.items():
        if not isinstance(value, np.ndarray):
            raise WeightsError(f"{name} is not a tensor")
        out[name] = value.astype(np.float32) if value.dtype.kind == "f" else value
    return out
