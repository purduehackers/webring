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
	handle        string
	discordUserId string
	url           string
}

type model struct {
	ring         []ring
	index        *template.Template
	ringModTime  int64
	indexModTime int64
}

// Pre-define all of our flags
var (
	flagListen         *string = flag.StringP("listen", "l", "127.0.0.1:2857", "Host and port go-webring will listen on")
	flagMembers        *string = flag.StringP("members", "m", "list.txt", "Path to list of webring members")
	flagIndex          *string = flag.StringP("index", "i", "index.html", "Path to home page template")
	flagContactString  *string = flag.StringP("contact", "c", "contact the admin and let them know what's up", "Contact instructions for errors")
	flagValidationLog  *string = flag.StringP("validationlog", "v", "validation.log", "Path to validation log, see docs for requirements")
	flagHost           *string = flag.StringP("host", "H", "", "Host this webring runs on, primarily used for validation")
	flagDiscordUrlFile *string = flag.StringP("discord-url-file", "d", "discord-webhook-url.txt", "Path to file containing Discord webhook URL")

	gDiscordUrl        *string = nil // Will be read from flagDiscordUrlFile
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

	fileHandler := http.StripPrefix("/static/", http.FileServer(http.Dir("static")))
	mux.Handle("/static/", fileHandler)

	if err := httpServer.ListenAndServe(); err == http.ErrServerClosed {
		log.Println("Web server closed")
	} else {
		log.Fatalln(err)
	}
}

func loadDiscordUrl() {
	discordUrlBytes, err := os.ReadFile(*flagDiscordUrlFile)
	if err != nil {
		log.Fatalln("Failed to read URL from file")
	}
	discordUrlString := strings.TrimSpace(string(discordUrlBytes))
	gDiscordUrl = &discordUrlString
}

func (m *model) init() {
	flag.Parse()
	log.Println("Listening on", *flagListen)
	if *flagHost == "" {
		log.Fatalln("Host flag is required")
	}
	log.Println("Looking for Discord webhook URL in", *flagDiscordUrlFile)
	loadDiscordUrl()
	log.Println("Looking for members in", *flagMembers)
	m.parseList()
	log.Println("Found", len(m.ring), "members")
	log.Println("Building homepage with", *flagIndex)
	m.parseIndex()
}
