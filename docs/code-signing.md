# Code signing policy

Windows installers of TifMrcView and TifMrcView Pro are built from this
repository by GitHub Actions (`.github/workflows/build.yml`) and signed in
that workflow.

Free code signing provided by [SignPath.io](https://signpath.io),
certificate by [SignPath Foundation](https://signpath.org).

## What is signed

Only binaries built by the workflow above from a commit on `main`: the
NSIS installers (`TifMrcView_<version>_x64-setup.exe`,
`TifMrcViewPro_<version>_x64-setup.exe`) and the programs inside them.
Installers built on a developer's own computer are never signed. Every
release is approved by hand before it is signed.

## Team roles

The project currently has one maintainer, who holds every role:

| Role | Members |
| --- | --- |
| Authors (commit without further review) | [Hao Li (@Leehow)](https://github.com/Leehow) |
| Reviewers (review every outside contribution) | [Hao Li (@Leehow)](https://github.com/Leehow) |
| Approvers (approve each signing request) | [Hao Li (@Leehow)](https://github.com/Leehow) |

All of them use multi-factor authentication for GitHub and SignPath.
Contributions from anyone else arrive as pull requests and are reviewed
before they are merged.

## Privacy policy

TifMrcView opens and displays files on your computer. It does not collect,
store or send your images, file names or any personal data. It talks to
the network only in these cases:

- **Update check.** A few seconds after start it downloads the small
  update feed from `dl.tifmrcview.com` / `tifmrcview.com` to see whether a
  newer version exists. Turn this off in the "Plugins and updates" dialog
  ("Check automatically at start"); "Check for updates" then runs it only
  when you click it. Updates are downloaded and installed only when you
  click "Update".
- **Plugin market.** When you open the plugin list (or the empty start
  window, which lists format plugins), it downloads the list of plugins
  from `tifmrcview.com`. A plugin is downloaded only when you install it.
- **TifMrcView Pro, segmentation (SAM).** The first time you use it, the
  model file is downloaded from Hugging Face (`huggingface.co`, or the
  mirror `hf-mirror.com`). Hugging Face's [privacy
  policy](https://huggingface.co/privacy) applies to that download.

These requests carry only what any web request carries (your IP address
and the app name and version). The download server keeps standard web access
logs, used only to count downloads and never shared. Otherwise this program will not transfer any information to other
networked systems unless specifically requested by the user.
