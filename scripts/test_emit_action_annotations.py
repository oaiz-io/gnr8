import importlib.util
import io
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('annotations', Path(__file__).with_name('emit-action-annotations.py'))
annotations = importlib.util.module_from_spec(spec)
spec.loader.exec_module(annotations)


def change(**updates):
    value = dict(kind='breaking', gating=True, code='request.property.required.added',
                 message='title became required', file='main.go', line=25,
                 span=dict(end_line=25))
    value.update(updates)
    return value


def render(changes, project='examples/bookstore', artifact='report', advisory=False):
    output = io.StringIO()
    annotations.emit(dict(schema_version=1, changes=changes), project, artifact, output, advisory)
    return output.getvalue()


class AnnotationTests(unittest.TestCase):
    def test_data_encoding_preserves_literal_escapes(self):
        self.assertEqual(annotations.encode_data('%0A\r\n::error,a'), '%250A%0D%0A::error,a')

    def test_property_encoding(self):
        self.assertEqual(annotations.encode_property('a:%0A,\r\nb'), 'a%3A%250A%2C%0D%0Ab')

    def test_exact_location_and_title(self):
        self.assertEqual(render([change()]), '::error file=examples/bookstore/main.go,line=25,endLine=25,title=gnr8%3A request.property.required.added::title became required\n')

    def test_levels_and_doc_silence(self):
        lines = render([change(), change(gating=False), change(kind='additive'), change(kind='doc_only')]).splitlines()
        self.assertEqual([line.split(' ')[0] for line in lines], ['::error', '::warning', '::notice'])
        self.assertEqual(render([change(kind='doc_only')]), '')

    def test_advisory_mode_uses_warnings_for_all_breaking_findings(self):
        lines = render([change(), change(gating=False), change(kind='additive')], advisory=True).splitlines()
        self.assertEqual([line.split(' ')[0] for line in lines], ['::warning', '::warning', '::notice'])

    def test_unanchorable_includes_removals_and_unknown_lines(self):
        text = render([change(file=None, line=None, span=None), change(line=None), change(line=0)])
        self.assertEqual(text, '::notice::gnr8: 3 further findings not annotated (3 unanchorable); see the job summary and the "report" artifact.\n')

    def test_omitted_location_fields_are_skipped_without_stopping_later_findings(self):
        removed = change(code='operation.removed')
        for key in ('file', 'line', 'span'):
            del removed[key]
        text = render([removed, change()])
        self.assertEqual(text.count('::error file='), 1)
        self.assertIn('1 further findings not annotated (1 unanchorable)', text)

    def test_cap_counts_all_omissions(self):
        text = render([change() for _ in range(60)] + [change(file=None, line=None)])
        self.assertEqual(text.count('::error '), 50)
        self.assertIn('11 further findings not annotated (1 unanchorable)', text)

    def test_path_is_one_normalized_join(self):
        self.assertIn('file=examples/bookstore/main.go,', render([change(file='./main.go')], 'examples/./bookstore'))

    def test_missing_span_does_not_invent_end_line(self):
        self.assertNotIn('endLine', render([change(span=None)]))

    def test_hostile_values_cannot_add_commands_or_properties(self):
        text = render([change(file='a,:\r\n::error title=evil', code='bad,code::\n',
                              message='%\n::error file=evil,title=evil::bad\rnext')],
                      artifact='a\n::error::bad')
        self.assertEqual(len(text.splitlines()), 1)
        self.assertIn('file=examples/bookstore/a%2C%3A%0D%0A%3A%3Aerror title=evil,', text)
        self.assertIn('title=gnr8%3A bad%2Ccode%3A%3A%0A::%25%0A::error', text)
        self.assertTrue(text.endswith('::bad%0Dnext\n'))

    def test_notice_encodes_artifact_name(self):
        text = render([change(file=None)], artifact='a\n::error::bad%')
        self.assertEqual(len(text.splitlines()), 1)
        self.assertIn('a%0A::error::bad%25', text)

    def test_unknown_schema_is_rejected(self):
        for version in (0, 2, True, '1'):
            with self.assertRaises(ValueError):
                annotations.emit(dict(schema_version=version, changes=[]), '.', 'report', io.StringIO())

    def test_invalid_span_is_rejected(self):
        with self.assertRaises(ValueError):
            render([change(span=dict(end_line=24))])


if __name__ == '__main__':
    unittest.main()
