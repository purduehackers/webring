// SPDX-FileCopyrightText: Amolith <amolith@secluded.site>
//
// SPDX-License-Identifier: BSD-2-Clause

package main

import (
	"html/template"
	"log/slog"
	"math/rand"
	"net/http"
	"time"
)

func (m *model) tryUpdateRing() {
	if m.modify("ring") {
		slog.Info("Ring modified; clearing field and re-parsing")
		m.parseList()
	}
}

// Serves the webpage created by createRoot()
func (m model) root(writer http.ResponseWriter, request *http.Request) {
	slog.Info("Request received", "url", request.URL) // NOCOMMIT
	if request.URL.Path != "/" {
		m.notFound(writer, request)
		return
	}
	m.tryUpdateRing()
	if m.modify("index") {
		slog.Info("Index modified; clearing field and re-parsing")
		m.parseIndex()
	}
	var table string
	for _, member := range m.ring {
		table = table + "  <tr>\n"
		table = table + "    <td>" + member.handle + "</td>\n"
		table = table + "    <td>" + link(member.url) + "</td>\n"
		table = table + "  </tr>\n"
	}
	writer.Header().Add("Content-Type", "text/html")
	err := m.index.Execute(writer, template.HTML(table))
	if err != nil {
		slog.Error("Error executing template", "error", err)
		http.Error(writer, "Internal server error", 500)
	}
}

// Redirects the visitor to the next member, wrapping around the list if the
// next would be out-of-bounds, and ensuring the destination returns a 200 OK
// status before performing the redirect.
func (m model) next(writer http.ResponseWriter, request *http.Request) {
	m.tryUpdateRing()
	host := request.URL.Query().Get("host")
	scheme := "https://"
	length := len(m.ring)
	for i, item := range m.ring {
		if item.url == host {
			for j := i + 1; j < length + i; j++ {
				dest := m.ring[j%length]
				destUrl := scheme + dest.url
				slog.Info("Checking site response", "site", destUrl)
				if is200(destUrl) {
					slog.Info("Redirecting visitor", "mode", "next", "from", host, "to", dest.url)
					http.Redirect(writer, request, destUrl, http.StatusFound)
					return
				}
				slog.Warn("Skipping site due to non-200 response", "site", dest.url)
			}
			http.Error(writer, `It would appear that either none of the ring members are accessible
(unlikely) or the backend is broken (more likely). In either case,
please `+*flagContactString, 500)
			return
		}
	}
	http.Error(writer, "Ring member '"+host+"' not found.", http.StatusNotFound)
}

// Redirects the visitor to the previous member, wrapping around the list if the
// next would be out-of-bounds, and ensuring the destination returns a 200 OK
// status before performing the redirect.
func (m model) previous(writer http.ResponseWriter, request *http.Request) {
	m.tryUpdateRing()
	host := request.URL.Query().Get("host")
	scheme := "https://"
	length := len(m.ring)
	for i, item := range m.ring {
		if item.url == host {
			for j := i - 1; j > i - length; j-- {
				dest := m.ring[(j + length) % length]
				destUrl := scheme + dest.url
				slog.Info("Checking site response", "site", destUrl)
				if is200(destUrl) {
					slog.Info("Redirecting visitor", "mode", "previous", "from", host, "to", dest.url)
					http.Redirect(writer, request, destUrl, http.StatusFound)
					return
				}
				slog.Warn("Skipping site due to non-200 response", "site", dest.url)
			}
			http.Error(writer, `It would appear that either none of the ring members are accessible
(unlikely) or the backend is broken (more likely). In either case,
please `+*flagContactString, 500)
			return
		}
	}
	http.Error(writer, "Ring member '"+host+"' not found.", http.StatusNotFound)
}

// Redirects the visitor to a random member
func (m model) random(writer http.ResponseWriter, request *http.Request) {
	m.tryUpdateRing()
	rand.Seed(time.Now().Unix())
	dest := m.ring[rand.Intn(len(m.ring)-1)]
	destUrl := "https://" + dest.url
	slog.Info("Redirecting visitor", "mode", "random", "to", dest.url)
	http.Redirect(writer, request, destUrl, http.StatusFound)
}

// Serves the log at *flagValidationLog
func (m model) validationLog(writer http.ResponseWriter, request *http.Request) {
	http.Header.Add(writer.Header(), "Content-Type", "text/plain")
	http.ServeFile(writer, request, *flagValidationLog)
}

func (m model) notFound(writer http.ResponseWriter, request *http.Request) {
	if m.notFoundHtml == nil {
		http.NotFound(writer, request)
	} else {
		if m.modify("404") {
			m.parse404()
		}
		writer.WriteHeader(http.StatusNotFound)
		writer.Header().Set("Content-Type", "text/html")
		writer.Write([]byte(*m.notFoundHtml))
	}
}
