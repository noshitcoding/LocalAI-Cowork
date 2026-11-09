"""image_engine.py: every pipeline step, formats, EXIF stripping, overlays, previews and limits."""
import io

from PIL import Image

import builders
from harness import EngineCase


def edit(case, steps, output='o.png', stage=None, inputs=None, code=None, **spec):
    stage = stage or case.stage()
    if not stage.inputs:
        for name, data in inputs or [('in.png', builders.image(200, 100))]:
            stage.add(name, data)
    spec = dict(spec, steps=steps)
    if code:
        return case.failed(stage, code, 'image_edit', 'pipeline', spec, output=output)
    receipt = case.ok(stage, 'image_edit', 'pipeline', spec, output=output)
    return receipt, Image.open(io.BytesIO(stage.read('out/' + output))), stage


class ImageStepTests(EngineCase):
    def test_geometry_steps(self):
        cases = [([{'op': 'crop', 'box': [10, 10, 110, 60]}], (100, 50)),
                 ([{'op': 'crop_relative', 'box': [0, 0, 0.5, 0.5]}], (100, 50)),
                 ([{'op': 'resize', 'width': 50}], (50, 25)),
                 ([{'op': 'resize', 'width': 50, 'height': 50}], (50, 25)),
                 ([{'op': 'resize', 'width': 50, 'height': 50, 'fit': 'cover'}], (50, 50)),
                 ([{'op': 'resize', 'width': 30, 'height': 70, 'fit': 'stretch'}], (30, 70)),
                 ([{'op': 'thumbnail', 'max_edge': 40}], (40, 20)),
                 ([{'op': 'rotate', 'degrees': 90}], (100, 200)),
                 ([{'op': 'rotate', 'degrees': 45, 'expand': False}], (200, 100)),
                 ([{'op': 'flip', 'direction': 'horizontal'}], (200, 100)),
                 ([{'op': 'pad', 'all': 10}], (220, 120)),
                 ([{'op': 'pad', 'left': 5, 'top': 1}], (205, 101)),
                 ([{'op': 'pad', 'size': [300, 300], 'color': 'black'}], (300, 300)),
                 ([{'op': 'border', 'width': 3, 'color': '#00FF00'}], (206, 106))]
        for steps, size in cases:
            receipt, image, _ = edit(self, steps)
            self.assertEqual(image.size, size, steps)
            self.assertEqual((receipt['result']['width'], receipt['result']['height']), size)
            self.assertEqual(receipt['result']['steps_applied'], len(steps))

    def test_flip_and_rotate_pixels(self):
        _, image, _ = edit(self, [{'op': 'flip', 'direction': 'horizontal'}])
        self.assertEqual(image.getpixel((199, 0)), (0, 0, 255))
        _, image, _ = edit(self, [{'op': 'rotate', 'degrees': 90}])
        self.assertEqual(image.getpixel((0, 199)), (0, 0, 255))  # counter-clockwise

    def test_color_steps(self):
        _, image, _ = edit(self, [{'op': 'grayscale'}])
        self.assertEqual(image.mode, 'L')
        _, image, _ = edit(self, [{'op': 'adjust', 'brightness': 0.5, 'contrast': 1.2, 'saturation': 0, 'sharpness': 2}])
        red, green, blue = image.getpixel((100, 50))
        self.assertEqual(red, green)  # saturation 0 -> gray
        _, image, _ = edit(self, [{'op': 'blur', 'radius': 3}, {'op': 'sharpen', 'amount': 1.5}])
        self.assertEqual(image.size, (200, 100))
        rgba = builders.image(50, 50, (255, 0, 0, 128), mode='RGBA')
        _, image, _ = edit(self, [{'op': 'adjust', 'brightness': 0.5}], inputs=[('a.png', rgba)])
        self.assertEqual(image.getpixel((25, 25))[3], 128)  # alpha untouched

    def test_annotations(self):
        receipt, image, _ = edit(self, [
            {'op': 'rectangle', 'box': [10, 10, 60, 60], 'color': 'yellow', 'width': 3, 'fill': '#0000FF80'},
            {'op': 'ellipse', 'box': [100, 10, 150, 60], 'color': '#00FF00'},
            {'op': 'arrow', 'from': [10, 90], 'to': [190, 90], 'color': 'white', 'width': 4},
            {'op': 'text', 'text': 'Hallo\nWelt', 'x': 100, 'y': 50, 'size': 20, 'color': 'black',
             'background': '#FFFFFFC0', 'anchor': 'center'}])
        self.assertEqual(image.getpixel((11, 11)), (255, 255, 0))
        fill = image.getpixel((35, 35))
        self.assertTrue(fill[2] > 100 and fill[0] > 90, fill)
        self.assertEqual(image.getpixel((150, 90)), (255, 255, 255))
        self.assertEqual(receipt['diagnostics'], [])
        receipt, _, _ = edit(self, [{'op': 'text', 'text': '中文'}])
        self.assertTrue(any('built-in font' in d['message'] for d in receipt['diagnostics']))

    def test_overlay(self):
        stage = self.stage()
        stage.add('base.png', builders.image(100, 100, (255, 255, 255)))
        stage.add('logo.png', builders.image(20, 20, (0, 0, 0, 255), mode='RGBA'))
        _, image, _ = edit(self, [{'op': 'overlay', 'input': 1, 'x': 90, 'y': -5, 'opacity': 0.5, 'scale': 2}],
                           stage=stage)
        self.assertEqual(image.size, (100, 100))
        gray = image.getpixel((95, 10))
        self.assertTrue(120 < gray[0] < 135, gray)
        self.assertEqual(image.getpixel((10, 50)), (255, 255, 255))

    def test_convert_modes_and_formats(self):
        for steps, output, mode, fmt in (([{'op': 'convert', 'mode': 'L'}], 'g.png', 'L', 'PNG'),
                                          ([{'op': 'convert', 'mode': 'RGBA'}], 'a.webp', None, 'WEBP'),
                                          ([{'op': 'convert', 'mode': 'P'}], 'p.png', 'P', 'PNG'),
                                          ([{'op': 'convert', 'mode': '1'}], 'b.bmp', '1', 'BMP'),
                                          ([{'op': 'convert', 'mode': 'CMYK'}], 'c.jpg', 'CMYK', 'JPEG'),
                                          ([{'op': 'grayscale'}], 'g.gif', None, 'GIF'),
                                          ([{'op': 'grayscale'}], 'g.tiff', 'L', 'TIFF')):
            receipt, image, _ = edit(self, steps, output=output)
            self.assertEqual(image.format, fmt, output)
            if mode:
                self.assertEqual(image.mode, mode, output)
            self.assertEqual(receipt['outputs'][0]['media_type'], 'image/' + fmt.lower())
        rgba = builders.image(20, 20, (255, 0, 0, 0), mode='RGBA')
        receipt, image, _ = edit(self, [{'op': 'flip', 'direction': 'vertical'}], output='flat.jpg',
                                 inputs=[('t.png', rgba)], background='#000000', quality=80)
        self.assertEqual(image.mode, 'RGB')
        self.assertLess(sum(image.getpixel((10, 10))), 30)
        self.assertTrue(any('transparency' in d['message'] for d in receipt['diagnostics']))
        edit(self, [{'op': 'grayscale'}], output='x.png', format='jpeg', code='invalid_spec')
        edit(self, [{'op': 'grayscale'}], output='x.svg', code='invalid_spec')


class ImageMetadataTests(EngineCase):
    def test_exif_is_removed_by_default(self):
        jpeg = builders.jpeg_with_exif(orientation=6)
        self.assertIn(b'SecretCam', jpeg)
        receipt, image, stage = edit(self, [{'op': 'auto_orient'}], output='o.jpg', inputs=[('p.jpg', jpeg)])
        data = stage.read('out/o.jpg')
        self.assertNotIn(b'SecretCam', data)
        self.assertNotIn(b'secret-location', data)
        self.assertEqual(dict(image.getexif()), {})
        self.assertEqual(image.size, (40, 60))
        self.assertFalse(receipt['result']['exif_kept'])

    def test_keep_exif_resets_orientation(self):
        receipt, image, stage = edit(self, [{'op': 'auto_orient'}], output='o.jpg', keep_exif=True,
                                     inputs=[('p.jpg', builders.jpeg_with_exif(orientation=6))])
        self.assertTrue(receipt['result']['exif_kept'])
        exif = image.getexif()
        self.assertEqual(exif.get(0x010F), 'SecretCam')
        self.assertEqual(exif.get(0x0112), 1)

    def test_png_text_chunks_removed(self):
        from PIL import PngImagePlugin
        info = PngImagePlugin.PngInfo()
        info.add_text('Comment', 'private-note')
        stage = self.stage()
        stage.add('a.png', builders.image(10, 10, pnginfo=info))
        _, _, stage = edit(self, [{'op': 'flip', 'direction': 'vertical'}], stage=stage)
        self.assertNotIn(b'private-note', stage.read('out/o.png'))


class ImageLimitTests(EngineCase):
    def test_preview(self):
        stage = self.stage()
        stage.add('in.png', builders.image(200, 100))
        receipt = self.ok(stage, 'image_edit', 'pipeline', {'steps': [{'op': 'resize', 'width': 3000}]},
                          output='big.png', options={'preview': True})
        preview = receipt['previews'][0]
        self.assertEqual(preview['name'], 'work/previews/image.png')
        self.assertEqual(max(preview['width'], preview['height']), 1600)

    def test_limits(self):
        edit(self, [{'op': 'grayscale'}] * 51, code='limit')
        edit(self, [{'op': 'resize', 'width': 100_000, 'height': 100_000, 'fit': 'stretch'}], code='limit')
        edit(self, [{'op': 'pad', 'all': 10_000}], code='limit')
        edit(self, [{'op': 'crop', 'box': [500, 500, 600, 600]}], code='invalid_spec')
        edit(self, [{'op': 'warp'}], code='invalid_spec')
        edit(self, [{'op': 'blur', 'radius': 2, 'sigma': 1}], code='invalid_spec')
        edit(self, [{'op': 'overlay', 'input': 3}], code='invalid_spec')
        receipt = edit(self, [{'op': 'grayscale'}], inputs=[('huge.png', builders.png_header(100_000, 100_000))],
                       code='limit')
        self.assertIn('pixel limit', receipt['error']['message'])
        edit(self, [{'op': 'grayscale'}], inputs=[('bomb.png', builders.png_header(8000, 8000))], code='limit')
        edit(self, [{'op': 'grayscale'}], inputs=[('x.png', b'not an image')], code='corrupt_input')
        stage = self.stage()
        stage.add('a.png', builders.image())
        stage.write('out/o.png', b'existing')
        code, receipt, _ = stage.run('image_edit', 'pipeline', {'steps': [{'op': 'grayscale'}]}, output='o.png')
        self.assertEqual((code, receipt['error']['code']), (2, 'invalid_spec'))
        self.assertEqual(stage.read('out/o.png'), b'existing')

    def test_crop_is_clipped_with_warning(self):
        receipt, image, _ = edit(self, [{'op': 'crop', 'box': [150, 50, 400, 300]}])
        self.assertEqual(image.size, (50, 50))
        self.assertTrue(any('clipped' in d['message'] for d in receipt['diagnostics']))
