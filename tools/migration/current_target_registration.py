"""Separately released literal registrations for immutable cold-target artifacts.

This file is intentionally outside each artifact's own producer_source_pins:
embedding an artifact SHA into a file pinned by that artifact creates a cycle.
The complete release receipt must independently bind this registry's bytes.
No dynamically discovered schema or unregistered directory entry is admitted.
"""
REGISTERED_PROFILES = {
    'v2-storage-75-complete-store-connect/1': {
        'manifest_file': 'current-target-75.json',
        'manifest_sha256': '9f69a559a70a7adf9c87d5d166c2270e8ee6d4288e8f792efcefdc2f2ecfc50c',
        'reference_directory': 'reference_source75',
    },
    'v2-storage-71-complete-store-connect/1': {
        'manifest_file': 'current-target-71.json',
        'manifest_sha256': '9a9ed9622c45ca0149f3a927dd5d5a3acb03d60bb07a3ea0f1bb6e007f3c1efd',
        'reference_directory': 'reference_source71',
    },
    'v2-storage-67-complete-store-connect/1': {
        'manifest_file': 'current-target-67.json',
        'manifest_sha256': '7d73781783626a3ec58a9e04173dc61c32f01a2f156ae09ac5fff52855695f49',
        'reference_directory': 'reference_source67',
    },
    'v2-storage-66-complete-store-connect/1': {
        'manifest_file': 'current-target-66.json',
        'manifest_sha256': 'b6aa739bf422a550a0de001df4617f5ab11e9e4ec02fb142daa46f58ee775ac7',
        'reference_directory': 'reference_source66',
    },
    'v2-storage-64-complete-store-connect/1': {
        'manifest_file': 'current-target-64.json',
        'manifest_sha256': '910bc94e96a5753160cc53ca525c90b1836fd37013b9063f6885796053d77aa3',
        'reference_directory': 'reference_source64',
    },
}
