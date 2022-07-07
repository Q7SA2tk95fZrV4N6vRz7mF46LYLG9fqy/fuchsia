// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

import 'package:fidl/fidl.dart' as fidl;
import 'package:fidl_fuchsia_io/fidl_async.dart';
import 'package:fidl_fuchsia_mem/fidl_async.dart';
import 'package:zircon/zircon.dart';

import 'pseudo_file.dart';

// ignore_for_file: unnecessary_null_comparison

/// Specifies how a VMO wrapped by [VmoFile] may be shared.
enum VmoSharingMode {
  /// The VMO may not be shared, the VMO file appears as a regular file.
  noSharing,

  /// A duplicate of the VMO is shared.
  shareDuplicate,

  // TODO(crjohns): Support cloning.
}

/// A node which wraps a VMO that can be duplicated when opened.
///
/// Reads when opening a [VmoFile] as a file are buffered, and repeated reads
/// will obtain the same information unless the connection is closed between
/// reads.
/// A duplicate of the underlying VMO is exposed through [Node.Describe] when
/// [VmoSharingMode] is [shareDuplicate].
class VmoFile extends PseudoFile {
  final Vmo _vmo;
  final VmoSharingMode _sharingMode;

  /// Constructor for read-only [Vmo]
  VmoFile.readOnly(this._vmo, this._sharingMode)
      : super.readOnly(() {
          int size = _vmo.getSize().size;
          return _vmo.read(size).bytesAsUint8List();
        });

  // TODO(crjohns): Support writable vmo files.

  Vmo? _getVmo() {
    if (_sharingMode == VmoSharingMode.shareDuplicate) {
      return _vmo.duplicate(ZX.RIGHTS_BASIC |
          ZX.RIGHT_READ |
          ZX.RIGHT_MAP |
          ZX.RIGHT_GET_PROPERTY);
    }
    return null;
  }

  @override
  Vmo getBackingMemory(VmoFlags flags) {
    final vmo = _getVmo();
    if (vmo != null) {
      return vmo;
    }
    throw fidl.MethodException(ZX.ERR_NOT_SUPPORTED);
  }

  @override
  NodeInfo describe() {
    final vmo = _getVmo();
    if (vmo != null) {
      return NodeInfo.withVmofile(
          Vmofile(vmo: vmo, offset: 0, length: vmo.getSize().size));
    }
    return NodeInfo.withFile(FileObject(event: null));
  }

  @override
  ConnectionInfo describe2(ConnectionInfoQuery query) {
    final vmo = _getVmo();
    if (vmo != null) {
      return ConnectionInfo(
          representation: Representation.withMemory(MemoryInfo(
              buffer: Range(vmo: vmo, offset: 0, size: vmo.getSize().size))));
    }
    return ConnectionInfo(representation: Representation.withFile(FileInfo()));
  }
}
