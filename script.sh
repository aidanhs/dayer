#!/bin/bash
set -o errexit
set -o pipefail
set -o nounset

JQ="$(dirname $0)/jq-1.5 --sort-keys"

CMD="$0"
SUBCMDHELP="Usage: $CMD [...]"

usage_analyse() {
    echo "$SUBCMDHELP analyse <savedir>"
}
op_analyse() {
    [ "$#" != 1 ] && usage_analyse && return 1
    local savedir="$1"
    local reposjson="$savedir/repositories"
    local repo
    local tag
    local layerid
    local layerids=""
    local repotags=""
    [ ! -d "$savedir" ] && echo "Save directory does not exist" && return 1
    [ ! -f "$reposjson" ] && echo "Could not find repositories json" && return 1

    echo "Identifying top layers"
    for repo in $($JQ -r 'keys | .[]' "$reposjson"); do
        echo "Repo: $repo"
        for tag in $(REPO="$repo" $JQ -r '.[env.REPO] | keys | .[]' "$reposjson"); do
            layerid="$(REPO="$repo" TAG="$tag" $JQ -r '.[env.REPO][env.TAG]' "$reposjson")"
            layerids="$layerids $layerid"
            echo "    Tag: $tag, ID: $layerid"
            repotags="$repotags $repo:$tag"
        done
    done

    echo
    echo "Checking parent layer ids"
    local layerdir
    local commonparentlayerid=""
    for layerid in $layerids; do
        layerdir="$savedir/$layerid"
        [ ! -f "$layerdir/VERSION" ] && echo "$layerdir/VERSION missing" && return 1
        [ "$(cat "$layerdir/VERSION")" != "1.0" ] && echo "$layerdir/VERSION is not 1.0" && return 1
        [ ! -f "$layerdir/json" ] && echo "$layerdir/json missing" && return 1
        parentlayerid="$(LAYER="$layerid" $JQ -r '.parent' "$layerdir/json")"
        if [ "$commonparentlayerid" = "" ]; then
            commonparentlayerid="$parentlayerid"
        fi
        [ "$commonparentlayerid" != "$parentlayerid" ] && echo "Parent mismatch for $layerid" && return 1
    done
    echo "All layers have a common parent: $commonparentlayerid"

    echo
    echo "Suggested commonisation:"
    local commonsep
    local tarpath
    for layerid in $layerids; do
        tarpath="$savedir/$layerid/layer.tar"
        [ ! -f "$tarpath" ] && echo "Could not find layer tar at $tarpath" && return 1
        echo "    $savedir/$layerid/layer.tar"
    done
    # TODO: check conflicts with the short prefixes
    commonsep="$(for layerid in $layerids; do echo "$layerid" | cut -c 1-9; done | xargs echo -n)"
    commonsep="$(echo "$commonsep" | xargs echo -n)"
    commonsep="$(echo "$commonsep" | sed 's/ /,/g')"
    echo "i.e. $savedir/{$commonsep}*/layer.tar"

    echo
    echo "Creating recombination commands:"
    echo '```'
    local randid="$(dd if=/dev/urandom bs=1 count=6 2>/dev/null | sha1sum | cut -c 1-9)"
    local dfile="Dockerfile_$randid"
    echo "docker tag $commonparentlayerid parenttmp_$randid"
    echo "echo -e 'FROM parenttmp_$randid\nCOPY common.tar /' > $dfile"
    echo "tar c $dfile common.tar | docker build -f $dfile --tag commontmp_$randid -"
    local repotag_i=0
    for repotag in $repotags; do
        echo "echo -e 'FROM commontmp_$randid\nCOPY individual_$repotag_i.tar /' > $dfile"
        echo "tar c $dfile individual_$repotag_i.tar | docker build -f $dfile --tag $repotag -"
        ((repotag_i=repotag_i+1))
    done
    echo "docker rmi commontmp_$randid parenttmp_$randid # just untagging"
    echo "rm $dfile"
    echo '```'
}

usage() {
    echo "Usage:"
    echo "    $CMD <operation>"
    echo "where operation is one of: analyse"
    echo
    usage_analyse
    echo
    exit 1
}

if [ $# = 0 ]; then
        usage
fi
#if ([ $# -gt 1 ] && [ "$1" = "-d" ]); then
#    if [ $# -lt 3 ]; then
#            usage
#    fi
#    VAL="$2"
#    shift 2
#    # process VAL
#fi


OP="$1"
shift
case "$OP" in
    analyse)
            op_analyse "$@"
            ;;
    *)
            usage
            ;;
esac
