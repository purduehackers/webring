// SPDX-FileCopyrightText: Amolith <amolith@secluded.site>
//
// SPDX-License-Identifier: BSD-2-Clause

package main

import (
	"html/template"
	"io/ioutil"
	"log/slog"
	"net/http"
	"os"
	"strings"
)

// Link returns an HTML, HTTPS link of a given URI
func link(l string) string {
	return "<a href='https://" + l + "'>" + l + "</a>"
}

// parseIndex parses the index template and returns a template struct.
func (m *model) parseIndex() {
	m.index = nil
	tmpl, err := template.ParseFiles(*flagIndex)
	if err != nil {
		slog.Error("Error parsing index template", "file", *flagIndex, "error", err)
		os.Exit(1)
	}
	m.index = tmpl
	tmplStat, err := os.Stat(*flagIndex)
	if err != nil {
		slog.Error("Error getting file metadata", "file", *flagIndex, "error", err)
		os.Exit(1)
	}
	m.indexModTime = tmplStat.ModTime().Unix()
}

// List parses the list of members, appends the data to a slice of type list,
// then returns the slice
func (m *model) parseList() {
	m.ring = nil
	file, err := ioutil.ReadFile(*flagMembers)
	if err != nil {
		slog.Error("Error loading webring member list", "error", err)
	}
	lines := strings.Split(string(file), "\n")
	for _, line := range lines[:len(lines)-1] {
		fields := strings.Fields(line)
		m.ring = append(m.ring, ring{handle: fields[0], discordUserId: fields[1], url: fields[2]})
	}
	fileStat, err := os.Stat(*flagMembers)
	if err != nil {
		slog.Error("Error getting file metadata", "file", *flagIndex, "error", err)
		os.Exit(1)
	}
	m.ringModTime = fileStat.ModTime().Unix()
}

// Modify takes arguments "index" or "ring" and returns true if either have been
// modified since last read
func (m *model) modify(a string) bool {
	if a == "ring" {
		ringStat, err := os.Stat(*flagMembers)
		if err != nil {
			slog.Error("Error getting file metadata", "file", *flagIndex, "error", err)
			os.Exit(1)
		}
		curRingModTime := ringStat.ModTime().Unix()
		return m.ringModTime < curRingModTime
	} else if a == "index" {
		indexStat, err := os.Stat(*flagIndex)
		if err != nil {
			slog.Error("Error getting file metadata", "file", *flagIndex, "error", err)
			os.Exit(1)
		}
		curIndexModTime := indexStat.ModTime().Unix()
		return m.indexModTime < curIndexModTime
	} else {
		slog.Error("modify() called with invalid argument", "arg", a)
		os.Exit(1)
	}
	return true
}

func is200(site string) bool {
	resp, err := http.Get(site)
	if err != nil {
		slog.Error("HTTP request failed", "site", site, "error", err)
		return false
	}
	if resp.StatusCode == http.StatusOK {
		return true
	}
	return false
}
