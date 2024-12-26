// SPDX-FileCopyrightText: Amolith <amolith@secluded.site>
//
// SPDX-License-Identifier: BSD-2-Clause

package main

import (
	"html/template"
	"log/slog"
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
	ring            []ring
	index           *template.Template
	notFoundHtml    *string
	notFoundModTime time.Time
	ringModTime     time.Time
	indexModTime    time.Time
}

// Pre-define all of our flags
var (
	flagListen         *string = flag.StringP("listen", "l", "127.0.0.1:2857", "Host and port go-webring will listen on")
	flagMembers        *string = flag.StringP("members", "m", "list.txt", "Path to list of webring members")
	flagIndex          *string = flag.StringP("index", "i", "index.html", "Path to home page template")
	flag404            *string = flag.StringP("404", "4", "404.html", "Path to HTML file to serve on 404")
	flagContactString  *string = flag.StringP("contact", "c", "contact the admin and let them know what's up", "Contact instructions for errors")
	flagValidationLog  *string = flag.StringP("validationlog", "v", "validation.log", "Path to validation log, see docs for requirements")
	flagHost           *string = flag.StringP("host", "H", "", "Host this webring runs on, primarily used for validation")
	flagDiscordUrlFile *string = flag.StringP("discord-url-file", "d", "discord-webhook-url.txt", "Path to file containing Discord webhook URL")

	gDiscordUrl        *string = nil // Will be read from flagDiscordUrlFile
)

func main() {
	logger := slog.NewTextHandler(os.Stdout, &slog.HandlerOptions{Level: slog.LevelInfo})
	slog.SetDefault(slog.New(logger))

	m := model{}
	m.init()

	mux := http.NewServeMux()

	// Ensure log file for member validation is in the current working
	// directory,
	if strings.HasPrefix(*flagValidationLog, "/") || strings.HasPrefix(*flagValidationLog, "..") {
		slog.Error("Validation log file must be in the current working directory")
		os.Exit(1)
	}

	// Ensure log file exists and if not, create it
	if _, err := os.Stat(*flagValidationLog); os.IsNotExist(err) {
		slog.Info("Validation log file does not exist; creating it")
		f, err := os.Create(*flagValidationLog)
		if err != nil {
			slog.Error("Error creating validation log file", "error", err)
			os.Exit(1)
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
		slog.Info("Web server closed")
	} else {
		slog.Error("Web server error", "error", err)
	}
}

func loadDiscordUrl() {
	discordUrlBytes, err := os.ReadFile(*flagDiscordUrlFile)
	if err != nil {
		slog.Error("Failed to read URL from file", "error", err)
		os.Exit(1)
	}
	discordUrlString := strings.TrimSpace(string(discordUrlBytes))
	gDiscordUrl = &discordUrlString
}

func (m *model) init() {
	flag.Parse()
	if *flagHost == "" {
		slog.Error("Host flag is required")
		os.Exit(1)
	}
	slog.Info("Listening", "address", *flagListen)
	slog.Info("Loading Discord webhook URL", "file", *flagDiscordUrlFile)
	loadDiscordUrl()
	slog.Info("Loading members", "file", *flagMembers)
	m.parseList()
	slog.Info("Loaded members", "member_count", len(m.ring))
	slog.Info("Building homepage", "file", *flagIndex)
	m.parseIndex()
	slog.Info("Reading 404 template", "file", *flag404)
	m.parse404()
}
