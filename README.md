dayer
=====

Eventually your go-to tool for manipulating Docker image layers.

commonise
---------

Takes layers from multiple images and creates a layer containing any common
files, with the goal of reducing the total size of all images.
This is useful for creating a shared base for Docker images not created with
layer-aware tools (e.g. Ansible).

The implementation is not yet a full featured subcommand, just a shell script to
walk you through the required steps.

First, save multiple big images (note the big layer must be at the top) and
extract them:

    $ docker save -o save.tar bigimage1 bigimage2 bigimage3 bigimage4
    $ mkdir layerdir
    $ tar -C layerdir -xf save.tar

(it's worth keeping the tar around just in case it goes horribly wrong and you
need to load your old images back in)

Now use the script to help you figure out what to pass to `dayer commonise-tar`:

    $ commonise.sh analyse layerdir
    Identifying top layers from layerdir/repositories
    Repo: bigimage1
        Tag: latest, ID: 2419dde0cfd992c34928ed6a3e3f94185d51d2ff4bd76bd118ea75b7afb3747d
    Repo: bigimage2
        Tag: latest, ID: 556f274dd6c3759db0a4b68d15c0f464dec68fe3535f4e5a846a6c03da2dc063
    Repo: bigimage3
        Tag: latest, ID: 8bb5900fa38ed9f02560cb4a282aa83e4884798d5411b04907c39ad33296c353
    Repo: bigimage4
        Tag: latest, ID: ff20133e3348b8c52334354297b019a5637e55894c8730695d1d7243c0a757e7

    Checking parent layer ids
    All layers have a common parent: 72703a0520b702adac8167f7aa25a8d2f58fe624937c16377e1a1b53a0519a86

    Suggested commonisation:
        layerdir/2419dde0cfd992c34928ed6a3e3f94185d51d2ff4bd76bd118ea75b7afb3747d/layer.tar
        layerdir/556f274dd6c3759db0a4b68d15c0f464dec68fe3535f4e5a846a6c03da2dc063/layer.tar
        layerdir/8bb5900fa38ed9f02560cb4a282aa83e4884798d5411b04907c39ad33296c353/layer.tar
        layerdir/ff20133e3348b8c52334354297b019a5637e55894c8730695d1d7243c0a757e7/layer.tar
    i.e. layerdir/{2419dde0c,556f274dd,8bb5900fa,ff20133e3}*/layer.tar

    Creating recombination commands:
    ```
    docker tag 72703a0520b702adac8167f7aa25a8d2f58fe624937c16377e1a1b53a0519a86 parenttmp_9f0d83ac7
    echo -e 'FROM parenttmp_9f0d83ac7\nADD common.tar /' > Dockerfile_9f0d83ac7
    tar c Dockerfile_9f0d83ac7 common.tar | docker build -f Dockerfile_9f0d83ac7 --tag commontmp_9f0d83ac7 -
    echo -e 'FROM commontmp_9f0d83ac7\nADD individual_0.tar /' > Dockerfile_9f0d83ac7
    tar c Dockerfile_9f0d83ac7 individual_0.tar | docker build -f Dockerfile_9f0d83ac7 --tag bigimage1 -
    echo -e 'FROM commontmp_9f0d83ac7\nADD individual_1.tar /' > Dockerfile_9f0d83ac7
    tar c Dockerfile_9f0d83ac7 individual_1.tar | docker build -f Dockerfile_9f0d83ac7 --tag bigimage2 -
    echo -e 'FROM commontmp_9f0d83ac7\nADD individual_2.tar /' > Dockerfile_9f0d83ac7
    tar c Dockerfile_9f0d83ac7 individual_2.tar | docker build -f Dockerfile_9f0d83ac7 --tag bigimage3 -
    echo -e 'FROM commontmp_9f0d83ac7\nADD individual_3.tar /' > Dockerfile_9f0d83ac7
    tar c Dockerfile_9f0d83ac7 individual_3.tar | docker build -f Dockerfile_9f0d83ac7 --tag bigimage4 -
    docker rmi commontmp_9f0d83ac7 parenttmp_9f0d83ac7 # just untagging
    rm Dockerfile_9f0d83ac7
    ```
    (you can just run script_9f0d83ac7.sh to recombine)

Pass the suggested tar arguments to `dayer commonise-tar`:

    $ dayer commonise-tar layerdir/{2419dde0c,556f274dd,8bb5900fa,ff20133e3}*/layer.tar
    Opening tars
    Loading layerdir/2419dde0cfd992c34928ed6a3e3f94185d51d2ff4bd76bd118ea75b7afb3747d/layer.tar
    Loading layerdir/2419dde0cfd992c34928ed6a3e3f94185d51d2ff4bd76bd118ea75b7afb3747d/layer.tar: found 37861 files, ignored 1935
    Loading layerdir/556f274dd6c3759db0a4b68d15c0f464dec68fe3535f4e5a846a6c03da2dc063/layer.tar
    Loading layerdir/556f274dd6c3759db0a4b68d15c0f464dec68fe3535f4e5a846a6c03da2dc063/layer.tar: found 32488 files, ignored 1967
    Loading layerdir/8bb5900fa38ed9f02560cb4a282aa83e4884798d5411b04907c39ad33296c353/layer.tar
    Loading layerdir/8bb5900fa38ed9f02560cb4a282aa83e4884798d5411b04907c39ad33296c353/layer.tar: found 60682 files, ignored 1946
    Loading layerdir/ff20133e3348b8c52334354297b019a5637e55894c8730695d1d7243c0a757e7/layer.tar
    Loading layerdir/ff20133e3348b8c52334354297b019a5637e55894c8730695d1d7243c0a757e7/layer.tar: found 36524 files, ignored 1912
    Phase 1: metadata compare
    Phase 1 complete: possible 12467 files with ~509MB
    Phase 2: data compare
    Phase 2 complete: actual 12465 files with ~509MB
    Phase 3a: preparing for layer creation
    Phase 3a complete
    Phase 3b: common layer creation
    Phase 3b complete: created common.tar
    Phase 3c: individual layer creation
    Phase 3c complete: created 4 individual tars

You'll now have the commonised tars:

    $ ls -hl | grep tar | grep -v save
    -rw-rw-r-- 1 aidanhs aidanhs 520M Oct 30 13:13 common.tar
    -rw-rw-r-- 1 aidanhs aidanhs 1.4G Oct 30 13:14 individual_0.tar
    -rw-rw-r-- 1 aidanhs aidanhs 2.1G Oct 30 13:15 individual_1.tar
    -rw-rw-r-- 1 aidanhs aidanhs 2.9G Oct 30 13:16 individual_2.tar
    -rw-rw-r-- 1 aidanhs aidanhs 2.3G Oct 30 13:17 individual_3.tar

So we're going to save about 1.5GB (uncompressed).

All that's left is to recombine the layers:

    $ ./script_9f0d83ac7.sh
    root@edasich:/space/ahobsons/rust/dayer# ./script_9f0d83ac7.sh
    + docker tag 72703a0520b702adac8167f7aa25a8d2f58fe624937c16377e1a1b53a0519a86 parenttmp_9f0d83ac7
    + echo -e 'FROM parenttmp_9f0d83ac7\nADD common.tar /'
    + tar c Dockerfile_9f0d83ac7 common.tar
    + docker build -f Dockerfile_9f0d83ac7 --tag commontmp_9f0d83ac7 -
    [...]
    Successfully built 3e4657f70d98
    + docker rmi commontmp_9f0d83ac7 parenttmp_9f0d83ac7
    Untagged: commontmp_9f0d83ac7:latest
    Untagged: parenttmp_9f0d83ac7:latest
    + rm Dockerfile_9f0d83ac7
    + rm script_9f0d83ac7.sh

Done! Your commonised images are tagged and ready to use!

Here's a concrete example of doing it with Ubuntu 14.04.2 and 14.04.3. Note we
had to find the layer with the actual data and make it the top layer.

```
$ docker pull ubuntu:14.04.2 && docker pull ubuntu:14.04.3
[...]
$ docker history ubuntu:14.04.2 | grep ADD
c0394f4c2a7a  [...]  /bin/sh -c #(nop) ADD file:0a5fd3a659be172e86   188.2 MB
$ docker history ubuntu:14.04.3 | grep ADD
48731f0a6276  [...]  /bin/sh -c #(nop) ADD file:ed2337b3477da68f9b   187.8 MB
$ docker tag c039 img1 && docker tag 4873 img2
$ docker save -o save.tar img1 img2
$ mkdir layerdir
$ tar -C layerdir -xf save.tar
$ ./commonise.sh analyse layerdir
[...]
All layers have a common parent: scratch
[...]
i.e. layerdir/{c0394f4c2,48731f0a6}*/layer.tar
[...]
(you can just run script_a358cb9f9.sh to recombine)
$ ./dayer commonise-tar layerdir/{c0394f4c2,48731f0a6}*/layer.tar
[...]
$ du -sh *.tar
133M    common.tar
56M     individual_0.tar
56M     individual_1.tar
376M    save.tar
$ ./script_a358cb9f9.sh
[...]
$ docker history img1
IMAGE               CREATED         [...]  SIZE
fee6e44c10a2        48 seconds ago  [...]  55.21 MB
d1c0d97eaeb4        52 seconds ago  [...]  133 MB
$ docker history img2
IMAGE               CREATED         [...]  SIZE
e6e7167342ff        46 seconds ago  [...]  54.81 MB
d1c0d97eaeb4        54 seconds ago  [...]  133 MB
```

commonise-tar
-------------

```
dayer commonise-tar TAR [TAR...]
```

Takes tarballs (it0...itN-1) and creates a common tarball (ct aka `common.tar`)
and individual tarballs (otX aka `individualX.tar`) such that extracting
ct and then otX on top is exactly the same as extracting itX.

More legibly: finds files shared across multiple tars, puts them in a single tar
and puts any leftover files into individual tars.

dev
---

```
DBG=0 && cargo build $([ $DBG = 0 ] && echo --release) && RUST_BACKTRACE=1 ./target/$([ $DBG = 0 ] && echo release || echo debug)/dayer commonise <tars>
```

Analysing layers:
```
$ # Anything with >= double digits of MB.
$ tar tvf 81f2400*/layer.tar | grep '[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9] '

$ # Size of files in tar directory. Assumes no filenames begin with a space. Make sure to have a trailing slash
$ PREFIX=opt/ && tar tvf 81f2400*/layer.tar "$PREFIX" | awk '{print $6, $3}' | sed 's|'^"$PREFIX"'\([^/]*\).* \([0-9][0-9]*\)$|\1 \2|' | awk '{a[$1]+=$2}END{for (i in a){print i,a[i]}}' sort
```

Github release:
```
$ cargo build --target=x86_64-unknown-linux-musl --release
```
