"""The fleet release is independent of management/operator rebuilds."""
import json

IMAGE_NAMES = {"server", "runner", "sandbox_core", "sandbox_gui", "sandbox_crew", "egress_proxy"}


def retain_fleet_images(images, run, validate):
    for image in sorted(set(images.values())):
        repository, digest = validate(image).split("@", 1)
        # Keep a named image reference as well as the digest used by workloads.
        # Replacing a sole build tag can otherwise remove it from containerd.
        run(["docker", "image", "tag", image, repository + ":retained-" + digest.replace(":", "-")])


def pinned_fleet_images(state, resolve, validate, write):
    marker = state / "image-set.json"
    previous = state / "config.json"
    if marker.is_symlink() or previous.is_symlink():
        raise ValueError("unsafe_fleet_image_set")
    if marker.exists():
        images = json.loads(marker.read_text())
    elif previous.exists():
        # Migrate the deployed fleet release, not today's mutable build tags.
        images = json.loads(previous.read_text())["images"]
    else:
        images = resolve()
    if not isinstance(images, dict) or set(images) != IMAGE_NAMES:
        raise ValueError("invalid_fleet_image_set")
    images = {name: validate(image) for name, image in images.items()}
    if not marker.exists():
        write(marker, json.dumps(images).encode())
    return images
