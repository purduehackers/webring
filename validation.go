// SPDX-FileCopyrightText: Amolith <amolith@secluded.site>
//
// SPDX-License-Identifier: BSD-2-Clause

package main

import (
	"bytes"
	"encoding/json"
	"fmt"
	"html"
	"io"
	"log"
	"net/http"
	"net/url"
	"os"
	"strings"
	"time"
)

// Loops through the list of members, checks whether they're up or down, whether
// they contain the requisite webring links, and appends any errors to the
// user-provided validation log
func (m *model) validateMembers() {
	// Get today's date with hours, minutes, and seconds
	today := time.Now().Format("2006-01-02")

	// Check the log header to see if we've already validated today
	logFile, err := os.Open(*flagValidationLog)
	if err != nil {
		fmt.Println("Error opening validation log:", err)
		logFile.Close()
		return
	}

	// Only read the most recent header, which is always 65 bytes long
	logHeader, err := io.ReadAll(io.LimitReader(logFile, 65))
	if err != nil {
		fmt.Println("Error reading validation log:", err)
		logFile.Close()
		return
	}

	if strings.Contains(string(logHeader), today) {
		logFile.Close()
		return
	}

	// Close the file so it's not locked while we're checking the members
	logFile.Close()

	// If any errors were found, write a report to the validation log
	errors := false

	// Count the numbers of notifications dispatched so we can print a log
	// message
	notifsSent := 0

	// Start the report with a header
	report := "===== BEGIN VALIDATION REPORT FOR " + today + " =====\n"

	for _, site := range m.ring {
		siteReportHeader := "- " + site.handle + " needs to fix the following issues on " + site.url + ":\n"
		issues := make([]string, 0)
		resp, err := http.Get("https://" + site.url)
		if err != nil {
			fmt.Println("Error checking", site.handle, "at", site.url, ":", err)
			issues = append(issues, "Error with site: " + err.Error())
			report += siteReportHeader
			report += formatIssues(issues)
			continue
		}

		if resp.StatusCode != http.StatusOK {
			issues = append(issues, "Site is not returning a 200 OK")
			report += siteReportHeader
			report += formatIssues(issues)
			resp.Body.Close()
			continue
		}

		// Read the response body into a string
		body, err := io.ReadAll(resp.Body)
		resp.Body.Close()
		if err != nil {
			fmt.Println("Error reading webpage body:", err)
			continue
		}

		requiredLinks := []string{
			"https://" + *flagHost + "/next?host=" + url.QueryEscape(site.url),
			"https://" + *flagHost + "/previous?host=" + url.QueryEscape(site.url),
			"https://" + *flagHost,
		}

		decodedBody := html.UnescapeString(string(body));
		for _, link := range requiredLinks {
			if !strings.Contains(decodedBody, link) {
				issues = append(issues, "Site is missing " + link)
			}
		}
		if len(issues) > 0 {
			report += siteReportHeader
			report += formatIssues(issues)
			if site.discordUserId != "-" {
				go notifyUser(site, issues)
				notifsSent += 1
			}
			errors = true
		}
	}

	report += "====== END VALIDATION REPORT FOR " + today + " ======\n\n"

	if errors {
		// Write the report to the beginning of the validation log
		f, err := os.OpenFile(*flagValidationLog, os.O_RDWR, 0o644)
		if err != nil {
			fmt.Println("Error opening validation log:", err)
			return
		}
		defer f.Close()

		logContents, err := io.ReadAll(f)
		if err != nil {
			fmt.Println("Error reading validation log:", err)
			return
		}

		if _, err := f.Seek(0, 0); err != nil {
			fmt.Println("Error seeking to beginning of validation log:", err)
			return
		}

		if _, err := f.Write([]byte(report)); err != nil {
			fmt.Println("Error writing to validation log:", err)
			return
		}

		if _, err := f.Write(logContents); err != nil {
			fmt.Println("Error writing to validation log:", err)
			return
		}
		log.Printf("Validation report for %s written", today)
		log.Printf("%d notifications dispatched", notifsSent)
	}
}

func formatIssues(issues []string) string {
	result := ""
	for _, issue := range issues {
		result += "  - "
		result += issue
		result += "\n"
	}
	return result
}

// Send a ping to the Discord user ID associated with the given site informing
// them of the given issues.
func notifyUser(site ring, issues []string) {
	issuesText := ""
	for _, issue := range issues {
		issuesText += "- "
		issuesText += issue
		issuesText += "\n"
	}
	// https://discord.com/developers/docs/resources/webhook#execute-webhook
	payload := map[string]interface{} {
		"content": fmt.Sprintf("<@%s>, please fix the following issues with your site's webring integration:\n%s", site.discordUserId, issuesText),
		"allowed_mentions": map[string]interface{} {
			"users": []string {site.discordUserId},
		},
	}
	payloadJson, err := json.Marshal(payload)
	if err != nil {
		fmt.Println("Error encoding JSON for Discord webhook request", err)
		return
	}

	resp, err := http.Post(*gDiscordUrl, "application/json", bytes.NewReader(payloadJson))
	if err != nil {
		fmt.Println("Error sending Discord webhook request", err)
		return
	}
	defer resp.Body.Close()

	if resp.StatusCode < 200 || resp.StatusCode > 299 {
		fmt.Println("Discord webhook returned status", resp.Status)
		return
	}
}
