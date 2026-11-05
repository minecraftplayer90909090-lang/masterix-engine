#!/usr/bin/env bash
# Masterix Engine rebrand (cs-engine ka fork). Repo ke root se chalao: bash engine-rebrand.sh
# Idempotent hai: kitni bhi baar chalao, nuksan nahi. Upstream merge ke baad dobara chalao.
set -u

ENGINE="Masterix Engine"      # "CS Engine" ki jagah
SHORT="Masterix"              # "CS" ki jagah
OWNER="AbhayTheMaster"        # "Craft Studio" ki jagah
EDITION="MASTERIX"
BRAND="masterix-engine"       # F3 me (version/brand) dikhta hai
LAUNCHER="Masterix Plus"      # "CS Launcher (Plus)" ki jagah
CODE="MX"                     # friend code prefix: CS-XXXXXX -> MX-XXXXXX (sirf screen par)

[ -d crates/cs-brand ] || { echo "ERROR: crates/cs-brand nahi mila. cs-engine repo ke root me chalao."; exit 1; }

BRANDRS=crates/cs-brand/src/lib.rs
FRIENDS=crates/horizon-client/src/cs_friends.rs
WF=.github/workflows/cs-engine.yml
TMP=$(mktemp)

echo "[1/4] Brand constants (cs-brand)..."
sed -i -E \
  -e "s|^(pub const NAME: &str = \")[^\"]*(\";)|\1$ENGINE\2|" \
  -e "s|^(pub const SHORT: &str = \")[^\"]*(\";)|\1$SHORT\2|" \
  -e "s|^(pub const VENDOR: &str = \")[^\"]*(\";)|\1$OWNER\2|" \
  -e "s|^(pub const TAGLINE: &str = \")[^\"]*(\";)|\1Powered by $OWNER\2|" \
  -e "s|^(pub const BRAND: &str = \")[^\"]*(\";)|\1$BRAND\2|" \
  -e "s|^(pub const EDITION: &str = \")[^\"]*(\";)|\1$EDITION\2|" \
  "$BRANDRS"

echo "[2/4] Text literals (Rust + build workflow; comments nahi chhute)..."
cat > "$TMP" <<SED
/^[[:space:]]*(\/\/|\*|\/\*|#)/b
s/CS Launcher Plus/$LAUNCHER/g
s/CS Launcher/$SHORT/g
s/CS Engine/$ENGINE/g
s/CS Friends/$SHORT Friends/g
s/CS voice/$SHORT voice/g
s/CS Voice/$SHORT Voice/g
s/\bCS (skins|skin|cape|server|host|control|HUD)/$SHORT \1/g
s/Powered by Craft Studio/Powered by $OWNER/g
s/Craft Studio/$OWNER/g
s/CS-XXXXXX/$CODE-XXXXXX/g
s/CS-ABCDEF/$CODE-ABCDEF/g
SED
find crates -type f -name '*.rs' -print0 | xargs -0 sed -i -E -f "$TMP"
[ -f "$WF" ] && sed -i -E -f "$TMP" "$WF"

echo "[3/4] Friend code: screen par MX-, server ko CS-..."
sed -i -E \
  -e 's/(code: s\(&[fr]\["code"\]\)),/\1.replacen("CS-", "MX-", 1),/' \
  -e 's/(me_code: s\(&me\["code"\]\)),/\1.replacen("CS-", "MX-", 1),/' \
  -e 's/assert_eq!\(s\.me_code, "CS-1"\)/assert_eq!(s.me_code, "MX-1")/' \
  "$FRIENDS"
grep -q 'replacen("MX-", "CS-", 1)' "$FRIENDS" || \
  sed -i '/let code: String = code\.trim()\.to_uppercase();/a\        let code = code.replacen("MX-", "CS-", 1);' "$FRIENDS"
rm -f "$TMP"

echo "[4/4] Report"
echo "  NAME     : $(grep -m1 '^pub const NAME' $BRANDRS)"
echo "  TAGLINE  : $(grep -m1 '^pub const TAGLINE' $BRANDRS)"
echo "  BRAND    : $(grep -m1 '^pub const BRAND' $BRANDRS)"
echo "  friends  : $(grep -c 'replacen' $FRIENDS) jagah code mapping"
echo "  bacha 'CS Engine/Friends/Craft Studio' (Rust+workflow, comments chhodke): $(grep -rnE 'CS Engine|CS Friends|Craft Studio|CS Launcher' crates $WF --include='*.rs' --include='*.yml' | grep -vE ':[0-9]+:[[:space:]]*(//|\*|/\*|#)' | wc -l)"
git diff --stat 2>/dev/null | tail -1
echo "Ho gaya. Ab: git diff --stat dekho, phir commit + push."
