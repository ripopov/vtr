#!/usr/bin/env python3
"""Export the elaborated slang AST into VTR's independent RTL VDB v2."""
from collections import Counter
import argparse
import hashlib
import json
from pathlib import Path
import shlex
import sys
import pyslang as slang


def tag(n):
    return str(n.kind).split('.')[-1]


class Exporter:
    def __init__(self, compilation):
        self.sm = compilation.sourceManager
        self.symbols = {}
        self.instances = []
        self.processes = []
        self.connections = []

    def owner(self, n):
        scope = n.parentScope
        instance = scope.containingInstance if scope else None
        return instance.hierarchicalPath if instance else None

    def loc(self, n):
        l = n.location if hasattr(n, 'location') else n.sourceRange.start
        return dict(file=str(self.sm.getFileName(l)), line=self.sm.getLineNumber(l),
                    column=self.sm.getColumnNumber(l))

    def typ(self, t):
        return dict(text=str(t), width=t.bitWidth, signed=t.isSigned,
                    four_state=t.isFourState, integral=t.isIntegral)

    def expr(self, e):
        d = dict(kind=tag(e), type=self.typ(e.type), source=self.loc(e))
        k = d['kind']
        if k in ('NamedValue', 'HierarchicalValue'):
            if isinstance(e.symbol, (slang.ast.ParameterSymbol, slang.ast.EnumValueSymbol)):
                d.update(kind='Constant', value=str(e.symbol.value))
            else:
                d['symbol'] = e.symbol.hierarchicalPath
        elif k in ('IntegerLiteral', 'UnbasedUnsizedIntegerLiteral'):
            d.update(kind='Constant', value=str(e.value))
        elif k in ('BinaryOp',):
            d.update(op=str(e.op).split('.')[-1], left=self.expr(e.left), right=self.expr(e.right))
        elif k in ('UnaryOp', 'Conversion'):
            d.update(operand=self.expr(e.operand))
            if k == 'UnaryOp':
                d['op'] = str(e.op).split('.')[-1]
        elif k == 'ConditionalOp' and len(e.conditions) == 1 and e.conditions[0].pattern is None:
            d.update(cond=self.expr(e.conditions[0].expr), yes=self.expr(e.left), no=self.expr(e.right))
        elif k == 'Concatenation':
            d['operands'] = [self.expr(x) for x in e.operands]
        elif k == 'Replication':
            d.update(count=int(str(e.count.constant)), operand=self.expr(e.concat))
        elif k == 'RangeSelect':
            r = e.value.type.getBitVectorRange()
            d.update(value=self.expr(e.value), left=self.expr(e.left), right=self.expr(e.right),
                     selection=str(e.selectionKind).split('.')[-1], range_left=r.left, range_right=r.right)
        elif k == 'ElementSelect':
            d.update(value=self.expr(e.value), selector=self.expr(e.selector))
            r = e.value.type.getBitVectorRange()
            d.update(range_left=r.left, range_right=r.right)
        else:
            d.update(kind='Unsupported', reason=f'expression {k}', reads=self.reads(e))
        return d

    def assignment(self, e):
        if tag(e) != 'Assignment' or e.isCompound or e.timingControl is not None:
            return dict(kind='Unsupported', reason='compound or timed assignment', source=self.loc(e))
        lhs = self.expr(e.left)
        if lhs['kind'] not in ('NamedValue', 'HierarchicalValue'):
            return dict(kind='Unsupported', reason='partial/aggregate assignment target', source=self.loc(e))
        return dict(kind='Assign', target=lhs['symbol'], value=self.expr(e.right),
                    nba=e.isNonBlocking, source=self.loc(e))

    def stmt(self, s):
        if s is None:
            return dict(kind='Empty')
        k = tag(s)
        if k == 'Block' and str(s.blockKind).endswith('Sequential'):
            return self.stmt(s.body)
        if k == 'List':
            return dict(kind='Sequence', statements=[self.stmt(x) for x in s.list])
        if k == 'Empty':
            return dict(kind='Empty')
        if k == 'ExpressionStatement':
            return self.assignment(s.expr)
        if k == 'Conditional' and len(s.conditions) == 1 and s.conditions[0].pattern is None:
            return dict(kind='If', cond=self.expr(s.conditions[0].expr), yes=self.stmt(s.ifTrue),
                        no=self.stmt(s.ifFalse), source=self.loc(s))
        return dict(kind='Unsupported', reason=f'statement {k}', source=self.loc(s))

    def targets(self, n):
        out = set()
        def visit(x):
            if isinstance(x, slang.ast.AssignmentExpression):
                def lhs(y):
                    if isinstance(y, (slang.ast.NamedValueExpression, slang.ast.HierarchicalValueExpression)):
                        out.add(y.symbol.hierarchicalPath)
                x.left.visit(lhs)
        n.visit(visit)
        return sorted(out)

    def reads(self, n):
        # Count references then subtract assignment LHS references; this preserves
        # self-feedback and reads hidden inside unsupported statements.
        refs = Counter()
        def ref(x, weight=1):
            if isinstance(x, (slang.ast.NamedValueExpression, slang.ast.HierarchicalValueExpression)):
                if not isinstance(x.symbol, (slang.ast.ParameterSymbol, slang.ast.EnumValueSymbol)):
                    refs[x.symbol.hierarchicalPath] += weight
        def visit(x):
            ref(x)
            if isinstance(x, slang.ast.AssignmentExpression):
                # Keep selectors on partial writes as reads, remove only the base.
                lhs = x.left
                while isinstance(lhs, (slang.ast.ElementSelectExpression, slang.ast.RangeSelectExpression)):
                    lhs = lhs.value
                if not x.isCompound:
                    ref(lhs, -1)
        n.visit(visit)
        return sorted(k for k, count in refs.items() if count > 0)

    def process(self, n):
        events = []
        if tag(n) == 'ContinuousAssign':
            body = self.assignment(n.assignment)
            mode = 'comb'
            if n.delay is not None:
                body = dict(kind='Unsupported', reason='continuous assignment delay', source=self.loc(n))
        else:
            s = n.body
            mode = 'comb' if str(n.procedureKind).endswith('AlwaysComb') else 'unsupported'
            if tag(s) == 'Timed':
                timing = s.timing
                if tag(timing) == 'ImplicitEvent':
                    mode = 'comb'
                else:
                    ts = timing.events if tag(timing) == 'EventList' else [timing]
                    mode = 'seq'
                    for t in ts:
                        if tag(t) != 'SignalEvent' or str(t.edge).split('.')[-1] not in ('PosEdge', 'NegEdge') or t.iffCondition is not None:
                            mode = 'unsupported'
                            break
                        events.append(dict(edge=str(t.edge).split('.')[-1], expr=self.expr(t.expr), source=self.loc(t)))
                s = s.stmt
            body = self.stmt(s)
        self.processes.append(dict(id=len(self.processes), owner=self.owner(n), origin="rtl", reads=self.reads(n), mode=mode, targets=self.targets(n),
                                   events=events, body=body, source=self.loc(n)))

    def visit(self, n):
        k = tag(n)
        if k in ('Variable', 'Net', 'Parameter', 'EnumValue'):
            d = dict(path=n.hierarchicalPath, owner=self.owner(n), kind=k, type=self.typ(n.type), source=self.loc(n))
            if k in ('Parameter', 'EnumValue'):
                d['value'] = str(n.value)
            self.symbols[n.hierarchicalPath] = d
            if k == 'Net' and n.initializer is not None:
                body = dict(kind='Assign', target=n.hierarchicalPath, value=self.expr(n.initializer), nba=False, source=self.loc(n))
                if n.delay is not None:
                    body = dict(kind='Unsupported', reason='net declaration delay', source=self.loc(n))
                self.processes.append(dict(id=len(self.processes), owner=self.owner(n), origin='rtl', reads=self.reads(n.initializer),
                                           mode='comb', targets=[n.hierarchicalPath], events=[], body=body, source=self.loc(n)))
            if k == 'Variable' and n.initializer is not None:
                self.processes.append(dict(id=len(self.processes), owner=self.owner(n), origin='rtl', reads=self.reads(n.initializer), mode='unsupported', targets=[n.hierarchicalPath], events=[], body=dict(kind='Unsupported', reason='variable initializer'), source=self.loc(n)))
        elif k == 'Instance':
            ports = []
            for p in n.body.portList:
                if hasattr(p, 'internalSymbol') and p.internalSymbol is not None:
                    ports.append(dict(name=p.name, symbol=p.internalSymbol.hierarchicalPath,
                                      direction=str(p.direction).split('.')[-1]))
                else:
                    raise RuntimeError(f'unsupported module port at {n.hierarchicalPath}.{p.name}')
            self.instances.append(dict(path=n.hierarchicalPath, parent=self.owner(n),
                                       ports=ports, definition=n.definition.name, source=self.loc(n)))
            for c in n.portConnections:
                p = c.port
                if p.internalSymbol is None or c.expression is None:
                    continue
                inside = p.internalSymbol.hierarchicalPath
                e = self.expr(c.expression)
                direction = str(p.direction).split('.')[-1]
                target, value = inside, e
                if direction in ('Out', 'InOut', 'Ref'):
                    # slang represents output connection as an assignment to the parent.
                    a = c.expression
                    if tag(a) == 'Assignment':
                        e = self.expr(a.left)
                        target = e.get('symbol', '')
                    else:
                        target = e.get('symbol', '')
                    value = dict(kind='NamedValue', symbol=inside, type=self.typ(p.type), source=self.loc(p))
                self.connections.append(dict(instance=n.hierarchicalPath, port=inside, direction=direction, expression=e, source=self.loc(p)))
                body = dict(kind='Assign', target=target, value=value, nba=False, source=self.loc(p))
                if direction not in ('In', 'Out') or not target:
                    body = dict(kind='Unsupported', reason='inout/ref or complex output connection', source=self.loc(p))
                targets = [target] if target else (self.targets(c.expression) or [inside])
                self.processes.append(dict(id=len(self.processes), owner=self.owner(n), origin='connection', reads=[], mode='comb', targets=targets, events=[], body=body, source=self.loc(p)))
        elif k in ('ContinuousAssign', 'ProceduralBlock'):
            self.process(n)


# --- Source index: classified tokens and declarations per file (docs/VDB_RTL.md, "Source index").
# A port of ext/verilator/src/vdb_index/SourceIndex.cpp; both producers must agree token for token.

INDEX_CLASSES = ['keyword', 'comment', 'number', 'string', 'operator', 'macro', 'variable', 'parameter',
                 'enumMember', 'type', 'module', 'interface', 'package', 'instance', 'function', 'property',
                 'namespace']
INDEX_MODIFIERS = ['declaration', 'input', 'output', 'inout', 'ref', 'clock', 'readonly', 'defaultLibrary',
                   'argument']
CLASS = {name: i for i, name in enumerate(INDEX_CLASSES)}
MOD = {name: 1 << i for i, name in enumerate(INDEX_MODIFIERS)}
DIRECTION_MOD = {'In': MOD['input'], 'Out': MOD['output'], 'InOut': MOD['inout'], 'Ref': MOD['ref']}
VALUE_KINDS = {'Variable', 'Net', 'ClockVar', 'LocalAssertionVar', 'Iterator', 'PatternVar'}
# Symbol kind -> token class; modifiers beyond `readonly` depend on the symbol and are added in classify_symbol.
SYMBOL_CLASSES = {}
SYMBOL_CLASSES.update(dict.fromkeys(['Parameter', 'TypeParameter', 'Genvar', 'Specparam', 'DefParam'], 'parameter'))
SYMBOL_CLASSES['EnumValue'] = 'enumMember'
SYMBOL_CLASSES.update(dict.fromkeys(VALUE_KINDS | {'FormalArgument', 'Port', 'MultiPort', 'ModportPort'}, 'variable'))
SYMBOL_CLASSES.update(dict.fromkeys(['InterfacePort', 'Instance', 'InstanceArray', 'PrimitiveInstance',
                                     'CheckerInstance'], 'instance'))
SYMBOL_CLASSES.update(dict.fromkeys(['Modport', 'ModportClocking'], 'interface'))
SYMBOL_CLASSES['Definition'] = 'module'
SYMBOL_CLASSES['Package'] = 'package'
SYMBOL_CLASSES.update(dict.fromkeys(
    ['TypeAlias', 'ForwardingTypedef', 'NetType', 'GenericClassDef', 'PredefinedIntegerType', 'ScalarType',
     'FloatingType', 'EnumType', 'PackedArrayType', 'FixedSizeUnpackedArrayType', 'DynamicArrayType',
     'DPIOpenArrayType', 'AssociativeArrayType', 'QueueType', 'PackedStructType', 'UnpackedStructType',
     'PackedUnionType', 'UnpackedUnionType', 'ClassType', 'CovergroupType', 'VoidType', 'NullType',
     'CHandleType', 'StringType', 'EventType', 'VirtualInterfaceType'], 'type'))
SYMBOL_CLASSES.update(dict.fromkeys(['Subroutine', 'MethodPrototype', 'LetDecl'], 'function'))
SYMBOL_CLASSES.update(dict.fromkeys(['Field', 'ClassProperty'], 'property'))
SYMBOL_CLASSES.update(dict.fromkeys(
    ['GenerateBlock', 'GenerateBlockArray', 'StatementBlock', 'ProceduralBlock', 'ClockingBlock', 'Sequence',
     'Property', 'Checker', 'CovergroupBody', 'Coverpoint', 'CoverCross', 'ConstraintBlock', 'CompilationUnit'],
    'namespace'))
# slang's LexerFacts::isKeyword: every *Keyword token kind plus 1step.
KEYWORD_TOKENS = {k for k in dir(slang.parsing.TokenKind) if k.endswith('Keyword')} | {'OneStep'}
LITERAL_TOKENS = {'IntegerLiteral', 'IntegerBase', 'UnbasedUnsizedLiteral', 'RealLiteral', 'TimeLiteral'}
MACRO_TOKENS = {'Directive', 'MacroUsage', 'MacroQuote', 'MacroTripleQuote', 'MacroEscapedQuote', 'MacroPaste',
                'EmptyMacroArgument'}
RAW_TRIVIA = {'Whitespace', 'EndOfLine', 'LineComment', 'BlockComment', 'DisabledText'}
COMMENT_TRIVIA = {'LineComment', 'BlockComment', 'DisabledText'}


def enum_name(value):
    return str(value).split('.')[-1]


def valid(loc):
    return loc is not None and loc.buffer.id != 0


def advance(loc, delta):
    return slang.SourceLocation(loc.buffer, loc.offset + delta)


def blen(text):
    return len(text.encode())


def is_ident_byte(c):
    return (c < 128 and chr(c).isalnum()) or c in b'_$'


def split_hierarchical_path(path):
    """`top.g[0].u.x` -> [('top', 'top'), ('g', 'top.g[0]'), ...]: (element name, path prefix)."""
    parts, start, depth = [], 0, 0
    for i, c in enumerate(path + '.'):
        depth += (c == '[') - (c == ']')
        if c == '.' and depth == 0:
            element = path[start:i]
            parts.append((element.partition('[')[0], path[:i]))
            start = i + 1
    return parts


class SourceIndexer:
    """Builds `source_index` from an elaborated compilation and its syntax trees."""

    def __init__(self, compilation, syntax_trees):
        self.c = compilation
        self.sm = compilation.sourceManager
        self.trees = syntax_trees
        self.texts = {}          # buffer id -> source bytes
        self.by_token = {}       # identifier SourceLocation -> [class, modifiers, declaration SourceLocation]
        self.clocks = set()      # declaration locations of edge-only event signals
        self.visited_bodies = set()
        self.files = []          # per file: buffer, previous_end, tokens as [line, column, length, class, modifiers, declaration]
        self.file_index = {}     # buffer id -> index in files
        self.expansions = set()
        self.includes = []       # (include directive location, included buffer)

    # -- Semantics: what the elaborated design says about identifier tokens.

    def text(self, buffer):
        if buffer.id not in self.texts:
            self.texts[buffer.id] = self.sm.getSourceText(buffer).encode()
        return self.texts[buffer.id]

    def in_file(self, loc):
        return valid(loc) and not self.sm.isMacroLoc(loc)

    def port_direction(self, symbol):
        # Stands in for ValueSymbol::getFirstPortBackref, which pyslang does not expose:
        # the port of the enclosing instance whose internal symbol this is.
        scope = symbol.parentScope
        body = scope.containingInstance if scope is not None else None
        port = body.findPort(symbol.name) if body is not None else None
        internal = getattr(port, 'internalSymbol', None)
        if internal is not None and internal.location == symbol.location and internal.name == symbol.name:
            return DIRECTION_MOD.get(enum_name(port.direction), 0)
        return 0

    def classify_symbol(self, symbol):
        kind = tag(symbol)
        cls = SYMBOL_CLASSES.get(kind)
        if cls is None:
            return None
        mods = 0
        if cls in ('parameter', 'enumMember'):
            mods = MOD['readonly']
        elif kind in VALUE_KINDS:
            mods = self.port_direction(symbol)
        elif kind == 'FormalArgument':
            mods = MOD['argument'] | DIRECTION_MOD.get(enum_name(symbol.direction), 0)
        elif kind in ('Port', 'MultiPort', 'ModportPort'):
            mods = DIRECTION_MOD.get(enum_name(symbol.direction), 0)
        elif kind == 'Definition' and enum_name(symbol.definitionKind) == 'Interface':
            cls = 'interface'
        return CLASS[cls], mods

    def record(self, token_loc, target, declaration):
        if not self.in_file(token_loc):
            return
        classification = self.classify_symbol(target)
        if classification is None:
            return
        cls, mods = classification
        if declaration:
            mods |= MOD['declaration']
        entry = self.by_token.get(token_loc)
        if entry is None:
            self.by_token[token_loc] = [cls, mods, target.location]
        else:
            entry[1] |= mods

    def declare(self, symbol):
        self.record(symbol.location, symbol, True)

    def reference(self, token, target):
        if token:
            self.record(token.location, target, target.location == token.location)

    @staticmethod
    def name_tokens(node, out):
        """Leaf identifier tokens of a name expression, left to right."""
        if node is None:
            return out
        k = tag(node)
        if k in ('IdentifierName', 'IdentifierSelectName', 'ClassName'):
            out.append(node.identifier)
        elif k == 'ScopedName':
            SourceIndexer.name_tokens(node.left, out)
            SourceIndexer.name_tokens(node.right, out)
        elif k == 'ElementSelectExpression' or k == 'InvocationExpression':
            SourceIndexer.name_tokens(node.left, out)
        elif k == 'MemberAccessExpression':
            SourceIndexer.name_tokens(node.left, out)
            out.append(node.name)
        return out

    def package_prefix(self, node):
        """`pkg::name`: the package prefix, when the name is scoped."""
        if node is None or tag(node) != 'ScopedName' or tag(node.left) != 'IdentifierName':
            return
        token = node.left.identifier
        package = self.c.getPackage(token.valueText)
        if package is not None:
            self.reference(token, package)

    def declared_type(self, declared):
        """Named type references in a declared type: `my_t x;` or `pkg::my_t x;`."""
        node = declared.typeSyntax if declared is not None else None
        if node is None or tag(node) != 'NamedType':
            return
        tokens = self.name_tokens(node.name, [])
        if not tokens:
            return
        self.package_prefix(node.name)
        typ = declared.type
        if tag(typ) in ('TypeAlias', 'TypeParameter'):
            self.reference(tokens[-1], typ)

    def name_before(self, end, name):
        """Sub-expressions of a name chain carry no syntax, only a range ending with the identifier."""
        if not self.in_file(end) or not name:
            return None
        text, offset, name = self.text(end.buffer), end.offset, name.encode()
        if offset < len(name) or offset > len(text) or text[offset - len(name):offset] != name:
            return None
        return slang.SourceLocation(end.buffer, offset - len(name))

    def reference_end(self, source_range, target):
        loc = self.name_before(source_range.end, target.name)
        if loc is not None:
            self.record(loc, target, target.location == loc)
        return loc

    @staticmethod
    def skip_blanks(text, pos):
        while pos > 0 and text[pos - 1] in b' \t':
            pos -= 1
        return pos

    @staticmethod
    def identifier_before(text, pos):
        """The identifier ending at `pos` after optional blanks: (name bytes, its start)."""
        pos = SourceIndexer.skip_blanks(text, pos)
        start = pos
        while start > 0 and is_ident_byte(text[start - 1]):
            start -= 1
        return text[start:pos], start

    def package_prefix_at(self, loc):
        """`pkg::name` written in the source: the package before the name at `loc`."""
        text = self.text(loc.buffer)
        pos = self.skip_blanks(text, loc.offset)
        if pos < 2 or text[pos - 2:pos] != b'::':
            return
        name, pos = self.identifier_before(text, pos - 2)
        package = self.c.getPackage(name.decode()) if name else None
        if package is not None:
            self.record(slang.SourceLocation(loc.buffer, pos), package, False)

    def hierarchical_prefix_at(self, loc, target):
        """`a.b[i].c` written in the source: the path elements before the name at `loc`, matched
        by name from the right. pyslang does not expose the resolved HierarchicalReference path,
        so the elements are the target's elaborated ancestors, looked up by hierarchical path."""
        text = self.text(loc.buffer)
        pos = loc.offset
        path = split_hierarchical_path(target.hierarchicalPath)
        element = len(path) - 1
        while element > 0:
            pos = self.skip_blanks(text, pos)
            if pos == 0 or text[pos - 1:pos] != b'.':
                return
            pos = self.skip_blanks(text, pos - 1)
            while pos > 0 and text[pos - 1:pos] == b']':
                depth = 0
                while True:
                    pos -= 1
                    depth += (text[pos:pos + 1] == b']') - (text[pos:pos + 1] == b'[')
                    if pos == 0 or depth <= 0:
                        break
                pos = self.skip_blanks(text, pos)
            name, pos = self.identifier_before(text, pos)
            if not name:
                return
            matched = False
            while element > 0:
                element -= 1
                if path[element][0] == name.decode():
                    symbol = self.c.getRoot().lookupName(path[element][1])
                    if symbol is not None:
                        self.record(slang.SourceLocation(loc.buffer, pos), symbol, False)
                    matched = True
                    break
            if not matched:
                return

    # -- Clocks: edge-sensitive event signals a process never reads as data.

    def collect_edges(self, timing, out):
        k = tag(timing)
        if k == 'SignalEvent':
            if timing.edge != slang.ast.EdgeKind.None_:
                symbol = timing.expr.getSymbolReference()
                if symbol is not None:
                    out.add(symbol.location)
        elif k == 'EventList':
            for event in timing.events:
                self.collect_edges(event, out)

    def find_clocks(self, block):
        body = block.body
        if tag(body) != 'Timed':
            return
        events = set()
        self.collect_edges(body.timing, events)
        if not events:
            return
        reads = set()
        def visit(x):
            if isinstance(x, (slang.ast.NamedValueExpression, slang.ast.HierarchicalValueExpression)):
                reads.add(x.symbol.location)
        body.stmt.visit(visit)
        self.clocks |= events - reads

    # -- Design walk: declarations and references, once per distinct parameterization.

    def body_key(self, inst):
        d = inst.definition
        key = [d.name, d.location.buffer.id, d.location.offset]
        for p in inst.body.parameters:
            if isinstance(p, slang.ast.ParameterSymbol):
                key.append(str(p.value))
            elif isinstance(p, slang.ast.TypeParameterSymbol):
                key.append(str(p.targetType.type))
            else:
                key.append('')
        return tuple(key)

    def imports(self, symbol):
        node = symbol.syntax
        if node is None or tag(node) != 'PackageImportItem':
            return
        package = self.c.getPackage(node.package.valueText)
        if package is not None:
            self.reference(node.package, package)
        if isinstance(symbol, slang.ast.ExplicitImportSymbol) and symbol.importedSymbol is not None:
            self.reference(node.item, symbol.importedSymbol)

    def interface_port(self, port):
        node = port.syntax
        if node is None or tag(node) != 'ImplicitAnsiPort':
            return
        header = node.header
        if header is not None and tag(header) == 'InterfacePortHeader' and port.interfaceDef is not None:
            self.reference(header.nameOrKeyword, port.interfaceDef)

    def instantiation(self, inst):
        """The module name, parameter overrides and port names of an instantiation."""
        node = inst.syntax
        if node is None or tag(node) != 'HierarchicalInstance':
            return
        parent = node.parent
        if parent is None or tag(parent) != 'HierarchyInstantiation':
            return
        self.reference(parent.type, inst.definition)
        if parent.parameters is not None:
            for assignment in parent.parameters.parameters:
                if isinstance(assignment, slang.syntax.SyntaxNode) and tag(assignment) == 'NamedParamAssignment':
                    name = assignment.name
                    param = next((p for p in inst.body.parameters if p.name == name.valueText), None)
                    if param is not None:
                        self.reference(name, param)
        for connection in node.connections:
            if isinstance(connection, slang.syntax.SyntaxNode) and tag(connection) == 'NamedPortConnection':
                name = connection.name
                port = next((p for p in inst.body.portList if p.name == name.valueText), None)
                if port is not None:
                    self.reference(name, port)

    def call(self, expr):
        if expr.isSystemCall:
            return
        subroutine = expr.subroutine
        if not isinstance(subroutine, slang.ast.Symbol):
            return
        tokens = self.name_tokens(expr.syntax, [])
        if tokens:
            self.reference(tokens[-1], subroutine)
        node = expr.syntax
        self.package_prefix(node.left if node is not None and tag(node) == 'InvocationExpression' else node)

    def visit_design(self, n):
        if isinstance(n, slang.ast.Symbol):
            if not isinstance(n, slang.ast.InstanceSymbol):
                self.declare(n)
            elif n.syntax is not None and tag(n.syntax) == 'HierarchicalInstance':
                # Top-level instances share the module name token with the definition.
                self.declare(n)
            if isinstance(n, slang.ast.ValueSymbol):
                self.declared_type(n.declaredType)
            if isinstance(n, slang.ast.TypeAliasType):
                self.declared_type(n.targetType)
            if isinstance(n, slang.ast.ProceduralBlockSymbol):
                self.find_clocks(n)
            if isinstance(n, (slang.ast.WildcardImportSymbol, slang.ast.ExplicitImportSymbol)):
                self.imports(n)
            if isinstance(n, slang.ast.InterfacePortSymbol):
                self.interface_port(n)
            if isinstance(n, slang.ast.InstanceSymbol):
                self.instantiation(n)
                key = self.body_key(n)
                if key in self.visited_bodies:
                    return slang.ast.VisitAction.Skip
                self.visited_bodies.add(key)
        elif isinstance(n, slang.ast.NamedValueExpression):
            loc = self.reference_end(n.sourceRange, n.symbol)
            if loc is not None:
                self.package_prefix_at(loc)
        elif isinstance(n, slang.ast.HierarchicalValueExpression):
            loc = self.reference_end(n.sourceRange, n.symbol)
            if loc is not None:
                self.hierarchical_prefix_at(loc, n.symbol)
        elif isinstance(n, slang.ast.ArbitrarySymbolExpression):
            loc = self.reference_end(n.sourceRange, n.symbol)
            if loc is not None:
                self.package_prefix_at(loc)
        elif isinstance(n, slang.ast.MemberAccessExpression):
            self.reference_end(n.sourceRange, n.member)
        elif isinstance(n, slang.ast.CallExpression):
            self.call(n)
        return None

    # -- Lexical walk: every token of every buffer, classified.

    def register_buffer(self, buffer):
        if buffer.id in self.file_index:
            return
        self.file_index[buffer.id] = len(self.files)
        self.files.append(dict(buffer=buffer, tokens=[], previous_end=0))
        included_from = self.sm.getIncludedFrom(buffer)
        if valid(included_from):
            self.includes.append((included_from, buffer))

    def locate(self, loc):
        if not self.in_file(loc) or loc.buffer.id not in self.file_index:
            return None
        return [self.file_index[loc.buffer.id], self.sm.getLineNumber(loc), self.sm.getColumnNumber(loc)]

    def included_buffer(self, start, end):
        """The buffer an include directive spanning [start, end) pulled in, if any."""
        for loc, buffer in self.includes:
            if loc.buffer == start.buffer and start.offset <= loc.offset < end.offset:
                return buffer
        return None

    def add(self, loc, text, cls, modifiers, declaration=None, sink=None):
        """Appends one token per line covered by `text` starting at `loc`."""
        file = self.file_index.get(loc.buffer.id)
        if file is None:
            return
        start = 0
        while start < len(text):
            end = text.find(b'\n', start)
            if end < 0:
                end = len(text)
            length = end - start
            if length > 0 and text[start + length - 1:start + length] == b'\r':
                length -= 1
            if length > 0:
                at = advance(loc, start)
                token = [self.sm.getLineNumber(at), self.sm.getColumnNumber(at), length, cls, modifiers, declaration]
                if sink is not None:
                    sink.append((file, token))
                else:
                    self.files[file]['tokens'].append(token)
            start = end + 1

    def classify(self, token, loc, directive):
        text = token.rawText
        if token.isMissing or not text:
            return
        kind = tag(token)
        cls, mods, declaration = None, 0, None
        if kind in KEYWORD_TOKENS:
            cls = CLASS['keyword']
        elif kind == 'Identifier':
            if directive:
                cls = CLASS['macro']
            elif token.location in self.by_token:
                cls, mods, declared = self.by_token[token.location]
                if declared in self.clocks:
                    mods |= MOD['clock']
                declaration = self.locate(declared)
        elif kind == 'SystemIdentifier':
            cls, mods = CLASS['function'], MOD['defaultLibrary']
        elif kind in ('StringLiteral', 'IncludeFileName'):
            cls = CLASS['string']
        elif kind in LITERAL_TOKENS:
            cls = CLASS['number']
        elif kind in MACRO_TOKENS:
            cls = CLASS['macro']
        elif kind not in ('EndOfFile', 'Unknown'):
            cls = CLASS['operator']
        if cls is not None:
            self.add(loc, text.encode(), cls, mods, declaration)

    def expansion(self, loc):
        """A macro expansion is shown as the macro usage the user wrote."""
        r = self.sm.getExpansionRange(loc)
        while valid(r.start) and self.sm.isMacroLoc(r.start):
            r = self.sm.getExpansionRange(r.start)
        if not valid(r.start) or r.start in self.expansions:
            return
        self.expansions.add(r.start)
        text = self.text(r.start.buffer)
        start, end = r.start.offset, min(r.end.offset, len(text))
        if end > start:
            self.add(r.start, text[start:end], CLASS['macro'], 0)

    def directive_tokens(self, node, start):
        """Tokens of a preprocessor directive are trivia of the token that follows; the directive's
        own leading comments are trivia of its first token, laid out from `start` when known."""
        cursor = start
        def visit(token):
            nonlocal cursor
            if not isinstance(token, slang.parsing.Token):
                return
            loc = token.location
            if not valid(loc) or self.sm.isMacroLoc(loc):
                return
            if cursor is not None and cursor.buffer == loc.buffer:
                self.comments(token, cursor)
            cursor = advance(loc, blen(token.rawText))
            self.classify(token, loc, True)
        node.visit(visit)

    def trivia_end(self, trivia):
        """Where a directive or skipped-token trivia ends, if that is a file location."""
        k = tag(trivia)
        if k == 'Directive':
            last = trivia.syntax().getLastToken()
        elif k == 'SkippedTokens':
            skipped = trivia.getSkippedTokens()
            if not skipped:
                return None
            last = skipped[-1]
        else:
            return None
        loc = last.location
        if not valid(loc) or self.sm.isMacroLoc(loc):
            return None
        return advance(loc, blen(last.rawText))

    def comments(self, token, cursor):
        """Emits the comments preceding `token`. Plain trivia carry no locations, so they are laid
        out forward from the previous token's end, following directives and explicit locations
        into other buffers; when that walk does not arrive exactly at the token, the trailing run
        of plain trivia is laid out backward from the token instead."""
        trivia = token.trivia
        if not trivia:
            return
        target = token.location
        pending, resume, consistent = [], [], True
        for item in trivia:
            k = tag(item)
            explicit = item.getExplicitLocation()
            if explicit is not None and not self.in_file(explicit):
                explicit = None
            if k == 'Directive':
                self.directive_tokens(item.syntax(), explicit)
            if explicit is not None:
                cursor = explicit
            if k in RAW_TRIVIA:
                raw = item.getRawText().encode()
                if k in COMMENT_TRIVIA:
                    self.add(cursor, raw, CLASS['comment'], 0, None, pending)
                cursor = advance(cursor, len(raw))
                if resume and cursor.offset >= len(self.text(cursor.buffer)):
                    cursor = resume.pop()
                continue
            end = self.trivia_end(item)
            if end is None:
                consistent = False
                break
            cursor = end
            if tag(item.syntax()) == 'IncludeDirective':
                included = self.included_buffer(item.syntax().getFirstToken().location, end)
                if included is not None:
                    resume.append(cursor)
                    cursor = slang.SourceLocation(included, 0)
        if consistent and cursor == target:
            for file, pending_token in pending:
                self.files[file]['tokens'].append(pending_token)
            return
        end = target.offset
        for item in reversed(trivia):
            k = tag(item)
            if k not in RAW_TRIVIA:
                break
            raw = item.getRawText().encode()
            if len(raw) > end:
                break
            end -= len(raw)
            if k in COMMENT_TRIVIA:
                self.add(slang.SourceLocation(target.buffer, end), raw, CLASS['comment'], 0)

    def token(self, token):
        loc = token.location
        if not valid(loc):
            return
        # Macro argument tokens keep the text the user wrote; expansions do not.
        while self.sm.isMacroArgLoc(loc):
            loc = self.sm.getOriginalLoc(loc)
        if self.sm.isMacroLoc(loc):
            self.expansion(loc)
            return
        file = self.file_index.get(loc.buffer.id)
        if file is None:
            return
        if loc == token.location:
            current = self.files[file]
            self.comments(token, slang.SourceLocation(current['buffer'], current['previous_end']))
            current['previous_end'] = loc.offset + blen(token.rawText)
        self.classify(token, loc, False)

    # -- Per-instance uninstantiated generate blocks, without descending into child instances.

    def inactive_blocks(self, scope, out):
        for member in scope:
            if isinstance(member, slang.ast.GenerateBlockArraySymbol):
                self.inactive_blocks(member, out)
            elif isinstance(member, slang.ast.GenerateBlockSymbol):
                if member.isUninstantiated:
                    out.append(member)
                else:
                    self.inactive_blocks(member, out)
        return out

    def inactive_ranges(self, blocks):
        ranges = []
        for block in blocks:
            node = block.syntax
            if node is None:
                continue
            r = node.sourceRange
            if not self.in_file(r.start) or r.start.buffer != r.end.buffer:
                continue
            file = self.file_index.get(r.start.buffer.id)
            if file is not None:
                ranges.append([file, self.sm.getLineNumber(r.start), self.sm.getColumnNumber(r.start),
                               self.sm.getLineNumber(r.end), self.sm.getColumnNumber(r.end)])
        return ranges

    def build(self, sources):
        """The `source_index` document; `sources` are the VDB `sources[].path` spellings."""
        root = self.c.getRoot()
        root.visit(self.visit_design)
        for definition in self.c.getDefinitions():
            self.declare(definition)
        for package in self.c.getPackages():
            self.declare(package)
        # Files are registered up front, in load order, so declaration locations resolve
        # regardless of which file is lexed first.
        for buffer in self.sm.getAllBuffers():
            path = str(self.sm.getFullPath(buffer))
            if self.sm.isFileLoc(slang.SourceLocation(buffer, 0)) and path and Path(path).is_file():
                self.register_buffer(buffer)
        for tree in self.trees:
            tree.root.visit(lambda n: self.token(n) if isinstance(n, slang.parsing.Token) else None)

        inactive = {}
        def visit_instance(n):
            if isinstance(n, slang.ast.InstanceSymbol):
                ranges = self.inactive_ranges(self.inactive_blocks(n.body, []))
                if ranges:
                    inactive[n.hierarchicalPath] = ranges
        root.visit(visit_instance)
        definitions = {}
        for symbol in list(self.c.getDefinitions()) + list(self.c.getPackages()):
            loc = self.locate(symbol.location)
            if loc is not None:
                definitions[symbol.name] = loc

        # Name files as the VDB does, so declarations join its symbol locations.
        work_dir = Path.cwd()
        known = {str((work_dir / source).resolve()): source for source in sources}
        files = []
        for file in self.files:
            full = Path(str(self.sm.getFullPath(file['buffer']))).resolve()
            path = known.get(str(full)) or (str(full.relative_to(work_dir)) if full.is_relative_to(work_dir) else str(full))
            tokens, seen = [], set()
            for token in sorted(file['tokens'], key=lambda t: (t[0], t[1])):
                if (token[0], token[1]) not in seen:
                    seen.add((token[0], token[1]))
                    tokens.append(token)
            declarations = [[i] + t[5] for i, t in enumerate(tokens) if t[5] is not None]
            files.append(dict(path=path, tokens=[x for t in tokens for x in t[:5]],
                              declarations=[x for d in declarations for x in d]))
        return dict(producer='slang ' + slang.__version__, classes=INDEX_CLASSES, modifiers=INDEX_MODIFIERS,
                    files=files, definitions=definitions, inactive=inactive)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('-o', '--output', required=True)
    p.add_argument('--top', required=True)
    p.add_argument('-I', dest='includes', action='append', default=[])
    p.add_argument('-D', dest='defines', action='append', default=[])
    p.add_argument('sources', nargs='+')
    args = p.parse_args()
    if slang.__version__ != '11.0.0':
        p.error('requires pyslang==11.0.0 (tools/vdb/requirements.txt)')
    driver = slang.driver.Driver()
    driver.addStandardArgs()
    cmd = ['slang', '--top', args.top] + ['-I'+x for x in args.includes] + ['-D'+x for x in args.defines] + args.sources
    if not driver.parseCommandLine(shlex.join(cmd)) or not driver.processOptions() or not driver.parseAllSources():
        raise RuntimeError('slang source loading failed')
    c = driver.createCompilation()
    root = c.getRoot()
    diagnostics = c.getAllDiagnostics()
    if diagnostics:
        print(slang.DiagnosticEngine.reportAll(c.sourceManager, diagnostics), file=sys.stderr)
    if c.hasIssuedErrors or c.hasFatalErrors:
        raise RuntimeError('slang elaboration failed')
    if [i.name for i in root.topInstances] != [args.top]:
        raise RuntimeError('requested top did not elaborate uniquely')
    ex = Exporter(c)
    root.visit(ex.visit)
    sources = []
    for b in c.sourceManager.getAllBuffers():
        name = str(c.sourceManager.getFullPath(b))
        if name and Path(name).is_file():
            sources.append(dict(path=str(Path(name).relative_to(Path.cwd())) if Path(name).is_relative_to(Path.cwd()) else name,
                                sha256=hashlib.sha256(Path(name).read_bytes()).hexdigest()))
    # Structured elaboration inputs shared with the Verilator producer: paths are as
    # given on the command line and resolve relative to work_dir.
    # work_dir '.' means relative paths resolve from the directory holding this VDB.
    elaboration = dict(work_dir='.', language='1800-2023', files=list(args.sources), library_files=[],
                       include_dirs=list(args.includes), library_exts=[],
                       defines=[dict(name=d.partition('=')[0], value=d.partition('=')[2]) for d in args.defines])
    doc = dict(format='vtr-rtl-vdb', version=2, producer='pyslang '+slang.__version__, top=args.top,
               sources=sorted(sources, key=lambda x:x['path']), options=cmd[1:], elaboration=elaboration,
               instances=ex.instances, symbols=ex.symbols, connections=ex.connections, processes=ex.processes)
    doc['design_id'] = hashlib.sha256(json.dumps(doc, sort_keys=True).encode()).hexdigest()
    # Not part of the identity: the index only renders what the hashed document already describes.
    doc['source_index'] = SourceIndexer(c, driver.syntaxTrees).build([s['path'] for s in sources])
    Path(args.output).write_text(json.dumps(doc, indent=2, sort_keys=True)+'\n')


if __name__ == '__main__':
    try:
        main()
    except (RuntimeError, OSError) as e:
        sys.exit(f'error: {e}')
