"""Page structure tests: self-contained, colours, cards, links, crates.

The page is rendered server-side (PR 2): these assertions pin the exact
HTML the renderer must produce. The visible criteria also carry manual
rows in docs/dashboard.md — a person opens the page in both themes.
"""

import json
import re

import page

# covers 30.2-a


def test_page_is_self_contained(repo_data, template_text):
    html = page.render_page(repo_data, template_text)
    assert not re.search(r"<script[^>]+src=", html)
    assert not re.search(r"<link\b", html)
    assert not re.search(r"<img[^>]+src=[\"']https?:", html)
    assert "@import" not in html
    assert "import(" not in html
    assert not re.search(r"url\(\s*[\"']?https?:", html)
    external = set(re.findall(r"<a[^>]+href=[\"'](https?://[^\"']+)", html))
    assert all(u.startswith("https://github.com/test/repo") for u in external)


# covers 30.2-b


def test_four_colours_and_text_badges_both_themes(repo_data, template_text):
    html = page.render_page(repo_data, template_text)
    assert ":root" in template_text
    for var in ("--ok", "--accent", "--warn", "--muted"):
        assert var in template_text
    assert "prefers-color-scheme: dark" in template_text
    badges = {
        "released": "in release",
        "develop": "in develop",
        "progress": "in progress",
        "planned": "planned",
        "unknown": "unknown",
    }
    for bucket, text in badges.items():
        assert f"lc-{bucket}" in html, bucket
        assert f">{text}</span>" in html, bucket


# covers 30.2-c


def test_stage_cards_counts_links_and_footer(repo_data, template_text):
    html = page.render_page(repo_data, template_text)
    assert 'id="stage-2"' in html
    assert 'id="stage-F1"' in html
    # stage 2: 1 released + 2 develop of 3 → exact segment widths
    assert 'class="seg seg-released" style="width:33.3%"' in html
    assert 'class="seg seg-develop" style="width:66.7%"' in html
    sha = repo_data["meta"]["target_sha"]
    assert f"https://github.com/test/repo/blob/{sha}/crates/x/tests/t.rs" in html
    assert "https://github.com/test/repo/pull/5" in html
    assert "[2.1-a]" in html
    assert repo_data["meta"]["release_tag"] in html
    assert sha in html
    assert repo_data["meta"]["generated_at"] in html
    assert "dashboard/1" in html


# covers 30.2-d


def test_crate_table_marks_and_violations(repo_data, template_text):
    html = page.render_page(repo_data, template_text)
    assert "ferrum-domain" in html
    assert "Domain." in html
    assert "changed since v0.1.0" in html
    assert "ferrum-domain → ferrum-io (layering: dependencies point inward)" in html
    assert "ferrum-app → wgpu (ferrum-app must compile without wgpu/egui)" in html


# covers 30.2-c


def test_page_carries_data_attributes(repo_data, template_text):
    """PR 3's interactivity reads the DOM; no duplicated JSON payload."""
    html = page.render_page(repo_data, template_text)
    assert 'data-stage="2" data-bucket="develop"' in html
    assert 'data-bucket="released"' in html
    assert 'data-crate="ferrum-io" data-changed="true"' in html
    assert "dashboard-data" not in html


# covers 30.2-c


def test_plan_text_is_html_escaped(repo_data, template_text):
    data = json.loads(json.dumps(repo_data))
    data["stages"][0]["stories"][0]["criteria"][0]["text"] = "<b>bold</b> & <script>"
    html = page.render_page(data, template_text)
    assert "&lt;b&gt;bold&lt;/b&gt; &amp; &lt;script&gt;" in html
    assert "<b>bold</b>" not in html


# covers 30.2-c


def test_warnings_are_shown(repo_data, template_text):
    html = page.render_page(repo_data, template_text)
    for warning in repo_data["meta"]["warnings"]:
        assert warning in html
