import argparse


def main(argv=None):
    parser = argparse.ArgumentParser(prog="tool")
    parser.add_argument("name", nargs="?", default="world")
    args = parser.parse_args(argv)
    print(f"hello, {args.name}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
