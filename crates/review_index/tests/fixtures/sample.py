class Calculator:
    def __init__(self, precision):
        self.precision = precision
    def add(self, a, b):
        return round(a + b, self.precision)

def standalone():
    pass
