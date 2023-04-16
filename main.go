// SPDX-FileCopyrightText: Amolith <amolith@secluded.site>
//
// SPDX-License-Identifier: BSD-2-Clause

package main

import (
	"html/template"
	"log"
	"net/http"
	"os"
	"strings"
	"time"

	flag "github.com/spf13/pflag"
)

type ring struct {
	handle string
	url    string
}

type model struct {
	ring         []ring
	index        *template.Template
	ringModTime  int64
	indexModTime int64
}

// Pre-define all of our flags
var (
	flagListen        *string = flag.StringP("listen", "l", "127.0.0.1:2857", "Host and port go-webring will listen on")
	flagMembers       *string = flag.StringP("members", "m", "list.txt", "Path to list of webring members")
	flagIndex         *string = flag.StringP("index", "i", "index.html", "Path to home page template")
	flagContactString *string = flag.StringP("contact", "c", "contact the admin and let them know what's up", "Contact instructions for errors")
	flagValidationLog *string = flag.StringP("validationlog", "v", "validation.log", "Path to validation log, see docs for requirements")
)

func main() {
	m := model{}
	m.init()

	mux := http.NewServeMux()

	// Ensure log file for member validation is in the current working
	// directory,
	if strings.HasPrefix(*flagValidationLog, "/") || strings.HasPrefix(*flagValidationLog, "..") {
		log.Fatalln("Validation log file must be in the current working directory")
	}

	// Ensure log file exists and if not, create it
	if _, err := os.Stat(*flagValidationLog); os.IsNotExist(err) {
		log.Println("Validation log file does not exist, creating")
		f, err := os.Create(*flagValidationLog)
		if err != nil {
			log.Fatalln("Error creating validation log file:", err)
		}
		f.Close()
	}

	// Spin off a goroutine to validate list members once a day
	go func() {
		for {
			m.validateMembers()
			time.Sleep(24 * time.Hour)
		}
	}()

	httpServer := &http.Server{
		Addr:    *flagListen,
		Handler: mux,
	}

	mux.HandleFunc("/", m.root)
	mux.HandleFunc("/next", m.next)
	mux.HandleFunc("/previous", m.previous)
	mux.HandleFunc("/random", m.random)
	mux.HandleFunc("/"+*flagValidationLog, m.validationLog)

	if err := httpServer.ListenAndServe(); err == http.ErrServerClosed {
		log.Println("Web server closed")
	} else {
		log.Fatalln(err)
	}
}

func (m *model) init() {
	flag.Parse()
	log.Println("Listening on", *flagListen)
	log.Println("Looking for members in", *flagMembers)
	m.parseList()
	log.Println("Found", len(m.ring), "members")
	log.Println("Building homepage with", *flagIndex)
	m.parseIndex()
}
