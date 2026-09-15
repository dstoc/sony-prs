#!/bin/sh
set -eu

destination=${1:-historical}
mkdir -p "$destination"

download() {
    url=$1
    filename=$2
    curl --fail --location --retry 3 --output "$destination/$filename" "$url"
}

download \
    https://projects.mobileread.com/reader/users/porkupan/PRS650/ebook_msc.1.06.zip \
    ebook_msc.1.06.zip
download \
    https://projects.mobileread.com/reader/users/porkupan/PRS350/tools/PRS350_update_tools.zip \
    PRS350_update_tools.zip
download \
    https://projects.mobileread.com/reader/users/porkupan/PRS350/flash_packages/PRS350.Flash.Package.orig.zip \
    PRS350.Flash.Package.orig.zip
download \
    https://wiki.mobileread.com/w/images/b/b2/Ebook_py_041.zip \
    Ebook_py_041.zip

if command -v git >/dev/null 2>&1 && [ ! -d "$destination/prs-plus/.git" ]; then
    git clone --filter=blob:none https://github.com/kartu/prs-plus.git "$destination/prs-plus"
fi

printf '%s\n' "Downloaded historical artifacts to $destination"
