// SPDX-FileCopyrightText: Amolith <amolith@secluded.site>
//
// SPDX-License-Identifier: BSD-2-Clause

package main

import (
	"fmt"
	"io/ioutil"
	"net/http"
	"net/url"
	"os"
	"strings"
	"time"
)

// Loops through the list of members, checks whether they're up or down, whether
// they contain the requisite fediring links, and appends any errors to the
// user-provided validation log
func (m *model) validateMembers() {
	// Get today's date with hours, minutes, and seconds
	today := time.Now().Format("2006-01-02")
	errors := false
	report := "===== BEGIN VALIDATION REPORT FOR " + today + " =====\n"

	for _, r := range m.ring {
		errorMember := false
		reportMember := ""
		resp, err := follow("https://" + r.url)
		if err != nil {
			fmt.Println("Error checking", r.handle, "at", r.url, ":", err)
			reportMember += "  - Error with site: " + err.Error() + "\n"
			if !errors {
				errors = true
			}
			report += "- " + r.handle + " needs to fix the following issues on " + r.url + ":\n"
			report += reportMember
			continue
		}

		if resp.StatusCode != http.StatusOK {
			reportMember += "  - Site is not returning a 200 OK\n"
			if !errors {
				errors = true
			}
			report += "- " + r.handle + " needs to fix the following issues on " + r.url + ":\n"
			report += reportMember
			continue
		}

		// Read the response body into a string
		body, err := ioutil.ReadAll(resp.Body)
		if err != nil {
			fmt.Println("Error reading webpage body:", err)
			continue
		}

		requiredLinks := []string{
			"https://fediring.net/next?host=" + url.QueryEscape(r.url),
			"https://fediring.net/previous?host=" + url.QueryEscape(r.url),
			"https://fediring.net",
		}

		for _, link := range requiredLinks {
			if !strings.Contains(string(body), link) {
				reportMember += "  - Site is missing " + link + "\n"
				if err != nil {
					fmt.Println("Error writing to validation log:", err)
					continue
				}
				if !errors {
					errors = true
				}
				if !errorMember {
					errorMember = true
				}
			}
		}
		if errorMember {
			report += "- " + r.handle + " needs to fix the following issues on " + r.url + ":\n"
			report += reportMember
		}

	}

	report += "====== END VALIDATION REPORT FOR " + today + " ======\n\n"

	fmt.Println(report)

	if errors {
		// Write the report to the beginning of the validation log
		f, err := os.OpenFile(*flagValidationLog, os.O_RDWR, 0o644)
		if err != nil {
			fmt.Println("Error opening validation log:", err)
			return
		}
		defer f.Close()

		logContents, err := ioutil.ReadAll(f)
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
		fmt.Println("Validation report for " + today + " written")
	}
}

// Recursively follows redirects until it finds a non-redirect response
func follow(url string) (*http.Response, error) {
	resp, err := http.Get(url)
	if err != nil {
		return nil, err
	}
	if resp.StatusCode == http.StatusMovedPermanently || resp.StatusCode == http.StatusFound || resp.StatusCode == http.StatusSeeOther || resp.StatusCode == http.StatusTemporaryRedirect || resp.StatusCode == http.StatusPermanentRedirect {
		return follow(resp.Header.Get("Location"))
	}
	return resp, nil
}
