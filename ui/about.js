"use strict";

// About dialog: app name, version and author. Edit AUTHOR to change what is
// shown; empty fields are left out. Links open in the system browser.

const AUTHOR = {
  name: "Hao Li",
  affiliation: "Institute of Chemistry, Chinese Academy of Sciences",
  email: "lihao9x@iccas.ac.cn",
  links: [
    // [label, url]; https:// and mailto: only.
    ["GitHub", "https://github.com/Leehow"],
    ["ORCID", "https://orcid.org/0009-0004-2696-3267"],
  ],
  // Shown under "引用" with a copy button; empty hides the section.
  citation: "Li, H. TifMrcView (software). https://github.com/Leehow/TifMrcView",
};

const REPO_URL = "https://github.com/Leehow/TifMrcView";
const SITE_URL = "https://tifmrcview.com/tifmrcview/";

function aboutLink(label, url) {
  const a = document.createElement("a");
  a.href = url;
  a.textContent = label;
  a.addEventListener("click", (e) => {
    e.preventDefault();
    invoke("open_link", { url }).catch((err) => toast(`无法打开链接：${err}`));
  });
  return a;
}

function renderAbout() {
  $("about-version").textContent = pluginHost.appVersion ? `v${pluginHost.appVersion}` : "";
  $("about-name").textContent = AUTHOR.name;
  $("about-aff").textContent = AUTHOR.affiliation;
  const links = $("about-links");
  links.replaceChildren();
  const all = [...AUTHOR.links];
  if (AUTHOR.email) all.push([AUTHOR.email, `mailto:${AUTHOR.email}`]);
  for (const [label, url] of all) links.append(aboutLink(label, url));
  const proj = $("about-project");
  proj.replaceChildren(aboutLink("官网", SITE_URL), aboutLink("GitHub", REPO_URL));
  $("about-cite-box").hidden = !AUTHOR.citation;
  $("about-cite").textContent = AUTHOR.citation;
}

function openAbout(force) {
  const dlg = $("about-dialog");
  const show = force ?? dlg.hidden;
  if (show) renderAbout();
  dlg.hidden = !show;
}

$("about-close").addEventListener("click", () => openAbout(false));
$("about-dialog").addEventListener("click", (e) => {
  if (e.target.id === "about-dialog") openAbout(false);
});
$("about-cite-copy").addEventListener("click", () =>
  navigator.clipboard.writeText(AUTHOR.citation).then(() => toast("已复制引用")),
);
window.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && !$("about-dialog").hidden) openAbout(false);
});
