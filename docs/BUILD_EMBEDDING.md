docs(build): record why a fresh userspace binary needs the kernel re-embedded

`interos.py build demo` can produce an image containing the previous shell
binary, with no error and a success line, because the kernel embeds each app's
ELF with `include_bytes!` and that does not by itself force the kernel to
rebuild. A change to an app can therefore be invisible in the image while the
artifact on disk is correct, which is indistinguishable from the change not
working.

This cost real debugging time on the System Space back control: the image held
an older launcher, so a tap that should have navigated did nothing, and the
natural conclusion was a bug in the handler. A diagnostic log added to the shell
went missing from the image while appearing in a grep of the source, which is
what identified the cause -- the two facts together are the check.

Two consequences recorded so they are not rediscovered:

- After changing an app, confirm the image holds the new build. The cheapest
  check that does not need a boot is to grep a freshly added literal out of
  `target/inter-os-bios.img`.
- `build_userspace.ps1` builds every app, so one app mid-edit blocks the image
  for everyone sharing the working tree. An app that does not compile is not a
  private problem.

No code behaviour changes.
