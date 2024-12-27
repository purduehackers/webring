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
	"log/slog"
	"net/http"
	"net/url"
	"os"
	"strconv"
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
		slog.Error("Error opening validation log", "error", err)
		logFile.Close()
		return
	}

	// Only read the most recent header, which is always 65 bytes long
	logHeader, err := io.ReadAll(io.LimitReader(logFile, 65))
	if err != nil {
		slog.Error("Error reading validation log", "error", err)
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
			slog.Error("Error fetching site", "user", site.handle, "url", site.url, "error", err)
			issues = append(issues, "Error with site: "+err.Error())
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
			slog.Error("Error reading webpage body", "error", err)
			continue
		}

		// Make sure the base link is last, since that's a substring of the
		// other two links, and we're tracking the match positions.
		requiredLinks := []string{
			"https://" + *flagHost + "/next?host=" + url.QueryEscape(site.url),
			"https://" + *flagHost + "/previous?host=" + url.QueryEscape(site.url),
			"https://" + *flagHost,
		}
		foundPositions := make([]int, len(requiredLinks))
		decodedBody := html.UnescapeString(string(body))
		for i, link := range requiredLinks {
			found := false
			offsetPos := 0
			for {
				pos := strings.Index(decodedBody[offsetPos:], link)
				if pos == -1 {
					break
				}
				// Check if this position has already been used
				absolutePos := offsetPos + pos
				positionUsed := false
				for _, usedPos := range foundPositions[:i] {
					if usedPos == absolutePos {
						positionUsed = true
						break
					}
				}
				if !positionUsed {
					foundPositions[i] = absolutePos
					found = true
					break
				}
				// Continue searching from after this match
				offsetPos += pos + len(link)
			}
			if !found {
				issues = append(issues, fmt.Sprintf("Site is missing <%s>", link))
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
			slog.Error("Error opening validation log", "error", err)
			return
		}
		defer f.Close()

		logContents, err := io.ReadAll(f)
		if err != nil {
			slog.Error("Error reading validation log", "error", err)
			return
		}

		if _, err := f.Seek(0, 0); err != nil {
			slog.Error("Error seeking to beginning of validation log", "error", err)
			return
		}

		if _, err := f.Write([]byte(report)); err != nil {
			slog.Error("Error writing to validation log", "error", err)
			return
		}

		if _, err := f.Write(logContents); err != nil {
			slog.Error("Error writing to validation log", "error", err)
			return
		}
		slog.Info("Validation report written", "date", today, "notifs_dispatched", notifsSent)
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

	// Logger which includes user info
	logger := slog.With(slog.Group("site", "user", site.handle, "discordId", site.discordUserId))

	// https://discord.com/developers/docs/resources/webhook#execute-webhook
	const SUPPRESS_EMBEDS int = 1 << 2
	payload := map[string]interface{}{
		"content": fmt.Sprintf("<@%s>, please fix the following issues with your site's webring integration:\n%s", site.discordUserId, issuesText),
		"allowed_mentions": map[string]interface{}{
			"users": []string{site.discordUserId},
		},
		"flags": SUPPRESS_EMBEDS,
	}
	payloadJson, err := json.Marshal(payload)
	if err != nil {
		logger.Error("Error creating JSON payload", "error", err)
		return
	}

	for {
		resp, err := http.Post(*gDiscordUrl, "application/json", bytes.NewReader(payloadJson))
		if err != nil {
			logger.Error("Error sending Discord webhook request", "error", err)
			return
		}
		defer resp.Body.Close()

		if resp.StatusCode == 429 {
			// We hit a rate limit. Wait according to the Retry-After header and try again.
			retryAfter := resp.Header.Get("Retry-After")
			if retryAfter == "" {
				logger.Error("Discord rate limit exceeded with no Retry-After header in response.")
				return
			}
			waitSeconds, err := strconv.ParseInt(retryAfter, 10, 64)
			if err != nil {
				logger.Error("Invalid Retry-After header in response", "header", "Retry-After: "+retryAfter)
				return
			}
			logger.Warn("Discord rate limit exceeded", "retry_after_seconds", waitSeconds)
			time.Sleep(time.Duration(waitSeconds) * time.Second)
		} else if resp.StatusCode < 200 || resp.StatusCode > 299 {
			logger.Error("Discord webhook returned non-2xx status", "code", resp.StatusCode, "status", resp.Status)
			body, err := io.ReadAll(resp.Body)
			if err != nil {
				logger.Error("Failed to read reponse body", "error", err)
			} else {
				logger.Debug("Discord webhook interaction failed",
					slog.Group("request", "body", payloadJson),
					slog.Group("response", "code", resp.StatusCode, "content_type",
						resp.Header.Get("Content-Type"), "body", string(body)))
			}
			return
		} else {
			// 2xx response
			break
		}
	}
}
