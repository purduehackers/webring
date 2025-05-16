package main

import "testing"

var site = ring{
	url: "my.site",
}

func TestMain(m *testing.M) {
	// Set the webring host
	host := "ring.host"
	flagHost = &host

	m.Run()
}

func TestFoundWithHost(t *testing.T) {
	html := `
	<a href="https://ring.host/">Index</a>
	<a href="https://ring.host/next?host=my.site">Next</a>
	<a href="https://ring.host/previous?host=my.site">Previous</a>
	`
	issues := checkHtmlForIssues(site, html)
	if len(issues) != 0 {
		t.Error("Expected no issues, found", len(issues))
	}
}

func TestFoundNoHost(t *testing.T) {
	html := `
	<a href="https://ring.host/">Index</a>
	<a href="https://ring.host/next">Next</a>
	<a href="https://ring.host/previous">Previous</a>
	`
	issues := checkHtmlForIssues(site, html)
	if len(issues) != 0 {
		t.Log("Expected no issues, found", len(issues))
		for _, issue := range issues {
			t.Log(issue)
		}
		t.Fail()
	}
}

// Ensures we don't greedily assume the first link is the index
func TestFoundIndexLast(t *testing.T) {
	html := `
	<a href="https://ring.host/next?host=my.site">Next</a>
	<a href="https://ring.host/previous?host=my.site">Previous</a>
	<a href="https://ring.host/">Index</a>
	`
	issues := checkHtmlForIssues(site, html)
	if len(issues) != 0 {
		t.Error("Expected no issues, found", len(issues))
	}
}

// Ensures our overlap checking works and that the prev/next links don't get double-counted as the index link
func TestNotFoundIndex(t *testing.T) {
	html := `
	<a href="https://ring.host/next?host=my.site">Next</a>
	<a href="https://ring.host/previous?host=my.site">Previous</a>
	`
	issues := checkHtmlForIssues(site, html)
	if len(issues) != 1 {
		t.Error("Expected 1 issue, found", len(issues))
	}
}
