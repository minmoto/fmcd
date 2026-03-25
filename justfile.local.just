dev:
  mprocs -c mprocs.yaml

dev-regtest:
  ./scripts/mprocs-nix.sh dev-fed mprocs-new.yaml

wscat:
  mprocs -c mprocs-wscat.yaml

publish:
  cargo publish

test-invoice amountMsat:
  ./scripts/mutinynet-invoice.sh {{amountMsat}}
