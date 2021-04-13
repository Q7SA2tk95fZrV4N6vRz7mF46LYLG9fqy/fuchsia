// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package fidlgen_cpp

import (
	"strings"

	"go.fuchsia.dev/fuchsia/tools/fidl/lib/fidlgen"
)

// namespaced is implemented by types that have a C++ namespace.
type namespaced interface {
	Namespace() namespace
}

// During template processing this holds the current namespace.
var currentNamespace namespace

//
// Predefined namespaces
//

func wireNamespace(library fidlgen.LibraryIdentifier) namespace {
	return unifiedNamespace(library).append("wire")
}

func naturalNamespace(library fidlgen.LibraryIdentifier) namespace {
	parts := libraryParts(library, changePartIfReserved)
	return namespace(parts)
}

func formatLibrary(library fidlgen.LibraryIdentifier, sep string, identifierTransform identifierTransform) string {
	name := strings.Join(libraryParts(library, identifierTransform), sep)
	return changeIfReserved(fidlgen.Identifier(name))
}

func unifiedNamespace(library fidlgen.LibraryIdentifier) namespace {
	return namespace([]string{formatLibrary(library, "_", keepPartIfReserved)})
}
