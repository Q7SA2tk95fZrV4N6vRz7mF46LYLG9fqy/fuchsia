// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package notice

import (
	"bufio"
	"bytes"
	"os"
	"strings"
)

// Flutter license files have the format:
//     ...
//     sectionDelimiter
//     ...
//     licenseDelimiter
//     license
//     appendixDelimiter?
//     appendix?
//     sectionDelimiter
//
//     sectionDelimiter
//     ...
//     licenseDelimiter
//     license
//     appendixDelimiter?
//     appendix?
//     sectionDelimiter
// etc.

var (
	sectionDelimiter  = []byte("====================================================================================================")
	licenseDelimiter  = []byte("----------------------------------------------------------------------------------------------------")
	appendixDelimiter = []byte("END OF TERMS AND CONDITIONS")
)

func ParseFlutter(path string) ([]*Data, error) {
	f, err := os.Open(path)
	if err != nil {
		return nil, err
	}
	defer f.Close()

	scanner := bufio.NewScanner(f)
	var (
		inAppendix bool
		inLicense  bool
		inSection  bool

		builder  strings.Builder
		licenses []*Data
	)

	license := &Data{}

	lineNumber := 0
	for scanner.Scan() {
		lineNumber = lineNumber + 1
		line := scanner.Bytes()
		if inSection {
			if inLicense {
				if bytes.Equal(line, appendixDelimiter) {
					inAppendix = true
				} else if bytes.Equal(line, sectionDelimiter) {
					inAppendix = false
					inLicense = false
					inSection = false
					license.LicenseText = []byte(builder.String())
					licenses = append(licenses, license)
					license = &Data{LineNumber: lineNumber}
					builder.Reset()
				} else if !inAppendix {
					builder.Write(line)
					builder.WriteString("\n")
				}
			} else if bytes.Equal(line, licenseDelimiter) {
				inLicense = true
			}
		} else if bytes.Equal(line, sectionDelimiter) {
			inSection = true
		}
	}
	if err := scanner.Err(); err != nil {
		return nil, err
	}

	return licenses, nil
}